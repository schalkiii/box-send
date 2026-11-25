//! 端到端：mock qBittorrent + mock NexusPHP（真实 TCP HTTP 服务）跑完整一轮 `run_round`。
//! 覆盖：辅种臂（检索命中 A 级 → 暂停推送 → 校验 100% → 启动）、
//! 转种草稿臂（站内没有 → 源种定位 → ReseedDraft）、已做种预筛（零请求）、
//! 索引落盘。HTTP 兼容补丁（hashes/hash 参数、[file]/[[file]] 回包）也在这里钉住。
//! 另覆盖：已存在 409 → 补挂本站 tracker、校验中跨轮收尾（满 → 启动 / 不满 → 回滚）、
//! 预算耗尽不记负缓存（格子跳过、下轮从头查）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bsend::bencode;
use bsend::config::{Config, CoverageSettings, DownloaderSettings, SiteConf};
use bsend::coverage::{run_round, RoundOptions};
use bsend::index::{
    ContentEntry, ContentIndex, CoverageStatus, SiteCoverage, SourceRecord, TorrentMeta,
};
use bsend::qbittorrent::QBittorrent;
use bsend::reseed::{review_reseed, run_reseed, run_reseed_reviewed, ReseedOutcome, ReseedRequest};
use bsend::sites::nexusphp::NexusPhp;
use bsend::sites::Framework;

// ---------- 极简 mock HTTP 服务 ----------

type Handler = Arc<dyn Fn(&str, &str, &[u8]) -> (u16, &'static str, Vec<u8>) + Send + Sync>;

/// 收下的请求（路径 + 请求体）——409 场景断言不出现 addTrackers / addTags
type CapturedRequests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

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
        302 => "Found",
        409 => "Conflict",
        _ => "Error",
    };
    // 3xx：payload 当 Location 用（登录墙场景的重定向）
    let location = if (300..400).contains(&status) {
        format!("Location: {}\r\n", String::from_utf8_lossy(&payload))
    } else {
        String::new()
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
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
const FILES_OTHER: &[(&str, i64)] = &[("b.mkv", 100), ("s/ch.srt", 5)];

fn qb_server(torrent_hash: String, tracker_hosts: Vec<String>) -> MockServer {
    qb_server_ex(torrent_hash, tracker_hosts, None, None)
}

/// 同 qb_server，但单查（hashes=）的进度/状态可编程——收尾场景要在两轮之间翻 mock 状态；
/// `add_capture` 给出时把 `torrents/add` 的请求体记下来（核对 savepath/contentLayout）。
fn qb_server_ex(
    torrent_hash: String,
    tracker_hosts: Vec<String>,
    progress: Option<Arc<Mutex<(f64, String)>>>,
    add_capture: Option<CapturedRequests>,
) -> MockServer {
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
            // 单查（?hashes=H）回单条（进度可编程，默认已完成）；全量回清单
            if path.contains("hashes=") {
                let (p, st) = progress
                    .as_ref()
                    .map(|m| m.lock().unwrap().clone())
                    .unwrap_or((1.0, "pausedUP".to_string()));
                let json = format!(
                    r#"[{{"hash":"{h}","progress":{p},"state":"{st}","up_limit":-1,"name":"Pack","save_path":"/data/"}}]"#
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
            if let Some(cap) = &add_capture {
                cap.lock().unwrap().push((path.to_string(), body.to_vec()));
            }
            return (200, "text/plain", b"Ok.".to_vec());
        }
        if path.starts_with("/api/v2/torrents/trackers") {
            return (200, "application/json", b"[]".to_vec());
        }
        // qB 4.x 形态：start/stop 不存在（404），启停走 resume/pause 兜底
        if path.starts_with("/api/v2/torrents/start") || path.starts_with("/api/v2/torrents/stop") {
            return (404, "text/plain", Vec::new());
        }
        // pause / resume / delete 等写操作兜底 200
        (200, "text/plain", Vec::new())
    }))
}

/// add 永远回 409（同 hash 种子已在下载器）：单 tracker 策略下不补挂、不改标签，
/// addTrackers / addTags 请求体收进 captured，断言侧要求这两类请求零出现。
fn qb_server_409(torrent_hash: String, captured: CapturedRequests) -> MockServer {
    let torrent_hash = Arc::new(torrent_hash);
    MockServer::start(Arc::new(move |_method, path, body| {
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
            if path.contains("hashes=") {
                let json = format!(
                    r#"[{{"hash":"{h}","progress":1.0,"state":"pausedUP","up_limit":-1}}]"#
                );
                return (200, "application/json", json.into_bytes());
            }
            let json = format!(
                r#"[{{"hash":"{h}","infohash_v2":"","name":"{NAME}","save_path":"/data/","category":"","tags":"","size":105,"progress":1.0,"tracker_hosts":[]}}]"#
            );
            return (200, "application/json", json.into_bytes());
        }
        if path.starts_with("/api/v2/torrents/files") {
            let json = r#"[{"name":"Pack/a.mkv","size":100,"index":0},{"name":"Pack/s/ch.srt","size":5,"index":1}]"#;
            return (200, "application/json", json.as_bytes().to_vec());
        }
        // addTrackers 要排在 add 前面（前缀会撞上）；仅记录，断言要求零请求
        if path.starts_with("/api/v2/torrents/addTrackers") {
            captured
                .lock()
                .unwrap()
                .push((path.to_string(), body.to_vec()));
            return (200, "text/plain", Vec::new());
        }
        // addTags 同样会被 `/add` 前缀命中；排在前面仅记录，断言要求零请求
        if path.starts_with("/api/v2/torrents/addTags") {
            captured
                .lock()
                .unwrap()
                .push((path.to_string(), body.to_vec()));
            return (200, "text/plain", Vec::new());
        }
        if path.starts_with("/api/v2/torrents/add") {
            return (409, "text/plain", Vec::new()); // qB 5.2+：已在下载器
        }
        if path.starts_with("/api/v2/torrents/trackers") {
            return (
                200,
                "application/json",
                br#"[{"url":"https://existing.example/announce"}]"#.to_vec(),
            );
        }
        // qB 4.x 形态：start/stop 不存在，启停走 resume/pause 兜底
        if path.starts_with("/api/v2/torrents/start") || path.starts_with("/api/v2/torrents/stop") {
            return (404, "text/plain", Vec::new());
        }
        (200, "text/plain", Vec::new())
    }))
}

/// qB 5.3 实机形态的下载器 mock：`/torrents/info` 只给 `tracker`（当前那条 announce），
/// `/app/version` 回纯文本版本号。
fn qb_server_single_tracker(torrent_hash: String, tracker_url: &str) -> MockServer {
    let torrent_hash = Arc::new(torrent_hash);
    let tracker = Arc::new(tracker_url.to_string());
    MockServer::start(Arc::new(move |_method, path, _body| {
        let h = &*torrent_hash;
        let u = &*tracker;
        if path.starts_with("/api/v2/auth/login") {
            return (200, "text/plain", b"Ok.".to_vec());
        }
        if path.starts_with("/api/v2/app/version") {
            return (200, "text/plain", b"v5.3.0".to_vec());
        }
        if path.starts_with("/api/v2/torrents/info") {
            if path.contains("hashes=") {
                let json = format!(
                    r#"[{{"hash":"{h}","progress":1.0,"state":"pausedUP","up_limit":-1}}]"#
                );
                return (200, "application/json", json.into_bytes());
            }
            let json = format!(
                r#"[{{"hash":"{h}","infohash_v2":"","name":"{NAME}","save_path":"/data/","category":"","tags":"","size":105,"progress":1.0,"tracker":"{u}"}}]"#
            );
            return (200, "application/json", json.into_bytes());
        }
        if path.starts_with("/api/v2/torrents/files") {
            let json = r#"[{"name":"Pack/a.mkv","size":100,"index":0},{"name":"Pack/s/ch.srt","size":5,"index":1}]"#;
            return (200, "application/json", json.as_bytes().to_vec());
        }
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
                announce_hosts: vec!["127.0.0.1".into()],
                announce_url: Some(format!(
                    "http://127.0.0.1:{pter_port}/announce?passkey=test"
                )),
            },
            SiteConf {
                id: "hdsky".into(),
                name: "天空".into(),
                url: format!("http://localhost:{hdsky_port}/"),
                framework: Framework::NexusPHP,
                search_url: None,
                api_key: None,
                announce_hosts: vec!["localhost".into()],
                announce_url: Some(format!(
                    "http://localhost:{hdsky_port}/announce?passkey=test"
                )),
            },
        ],
        coverage: CoverageSettings {
            per_site_interval_sec: 0.0,
            jitter_sec: 0.0,
            verify_wait_sec: 3,
            verify_poll_sec: 1,
            // 限流最低间隔压到 1s：退避数学在单测里钉，e2e 不必真等 60 秒
            rate_limit_floor_sec: 1,
            ..CoverageSettings::default()
        },
        user_agent: "bsend-e2e-test".into(),
        cookies: HashMap::new(),
        cookie_cloud: None,
        skipped_sites: Vec::new(),
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
        qb.requests("POST /api/v2/auth/login").len(),
        1,
        "登录一次就够（SID 复用，不每个请求都登）"
    );
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

/// 场景四：C 级（同名、总大小一致但清单拼不上）→ 待人工确认，绝不动下载器。
#[test]
fn e2e_name_only_goes_to_review() {
    let dir = temp_dir("review");
    // 总大小必须与本地一致（105）：总大小不同已属「其他版本」，不再进人工队列
    let t_site = Arc::new(torrent_bytes(
        "Pack",
        &[("a.mkv", 100), ("s/ch.srt", 4), ("extra.nfo", 1)],
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

/// 场景四之二：同名但总大小不同（分集 vs 全集、1080p vs 2160p）→ 其他版本，
/// 不进人工确认队列，也不动下载器。
#[test]
fn e2e_size_diff_goes_to_other_version() {
    let dir = temp_dir("size_diff");
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
        CoverageStatus::OtherVersion,
        "总大小不同应判其他版本"
    );
    assert!(entry.coverage["pter"]
        .note
        .as_deref()
        .unwrap()
        .contains("不是同一段文件"));
    assert_eq!(
        qb.requests("POST /api/v2/torrents/add").len(),
        0,
        "其他版本绝不动下载器"
    );
}

/// 场景五：add 回 409（同 hash 种子已在下载器）→ 不算失败；单 tracker 策略下
/// 不改动原种子、不补挂任何 tracker，只记「已存在、已由外部工具覆盖」。
#[test]
fn e2e_already_exists_does_not_touch_original_torrent() {
    let dir = temp_dir("exists409");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let captured: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_409(hash, captured.clone());
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
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();

    let entry = index.contents.values().next().unwrap();
    assert_eq!(
        entry.coverage["pter"].status,
        CoverageStatus::Assisted,
        "已存在也算辅种落地"
    );
    let note = entry.coverage["pter"].note.as_deref().unwrap();
    assert!(note.contains("已存在"), "note = {note}");
    assert!(note.contains("单 tracker"), "note = {note}");
    // 单 tracker 策略：不补挂、不改标签，原种子保持原样
    let adds = captured.lock().unwrap();
    let tracker_reqs: Vec<_> = adds
        .iter()
        .filter(|(p, _)| p.contains("addTrackers"))
        .collect();
    let tag_reqs: Vec<_> = adds.iter().filter(|(p, _)| p.contains("addTags")).collect();
    assert_eq!(tracker_reqs.len(), 0, "不允许 addTrackers");
    assert_eq!(tag_reqs.len(), 0, "不允许改原种子标签");
    drop(adds);
    // 已存在路径不动启停：那条种子本来就在跑
    assert_eq!(
        qb.requests("POST /api/v2/torrents/resume").len(),
        0,
        "已存在的种子不碰启停"
    );
}

/// 场景六：校验迟迟不结束 → 落「校验中」+ verify_hash 持久化；
/// 下一轮收尾 pass 直查下载器（零站点请求）：校验满 → 启动 → 已辅。
#[test]
fn e2e_verifying_collect_assisted() {
    let dir = temp_dir("verify-ok");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let progress = Arc::new(Mutex::new((0.0f64, "checkingFiles".to_string())));
    let qb = qb_server_ex(hash, Vec::new(), Some(progress.clone()), None);
    let pter = nexus_site_server("hit", t.clone());
    let hdsky = nexus_site_server("miss", t.clone());
    let mut cfg = test_config(qb.port, pter.port, hdsky.port, &dir);
    cfg.coverage.verify_wait_sec = 2;
    cfg.coverage.verify_poll_sec = 1;

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let _ = run_round(
        &cfg,
        qb_client.clone(),
        &ads,
        &mut index,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();

    // 第一轮：等到头没结论 → 校验中；收尾凭据必须落盘（否则重启就没人收尾了）
    let entry = index.contents.values().next().unwrap();
    assert_eq!(entry.coverage["pter"].status, CoverageStatus::Verifying);
    assert!(
        entry.coverage["pter"].verify_hash.is_some(),
        "verify_hash 是下一轮收尾的凭据"
    );
    let idx2 = ContentIndex::load(&cfg.coverage_state_path());
    let e2 = idx2.contents.values().next().unwrap();
    assert_eq!(e2.coverage["pter"].status, CoverageStatus::Verifying);
    assert!(
        e2.coverage["pter"].verify_hash.is_some(),
        "磁盘上也要有凭据"
    );

    // 第二轮：翻成校验完成 → 收尾 pass 启动它 → 已辅；全程零站点请求
    *progress.lock().unwrap() = (1.0, "pausedUP".to_string());
    let site_before =
        pter.requests("GET /torrents.php").len() + hdsky.requests("GET /torrents.php").len();
    let mut index2 = ContentIndex::load(&cfg.coverage_state_path());
    let report2 = run_round(
        &cfg,
        qb_client,
        &ads,
        &mut index2,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();
    let site_after =
        pter.requests("GET /torrents.php").len() + hdsky.requests("GET /torrents.php").len();
    assert_eq!(site_after, site_before, "收尾不花站点请求");
    assert_eq!(report2.by_status[&CoverageStatus::Assisted], 1);
    assert_eq!(
        qb.requests("POST /api/v2/torrents/resume").len(),
        2,
        "推送时 start 一次触发校验 + 收尾时再 start 一次"
    );
    let e3 = index2.contents.values().next().unwrap();
    assert_eq!(e3.coverage["pter"].status, CoverageStatus::Assisted);
    assert!(
        e3.coverage["pter"]
            .note
            .as_deref()
            .unwrap()
            .contains("收尾"),
        "note = {:?}",
        e3.coverage["pter"].note
    );
    assert!(e3.coverage["pter"].verify_hash.is_none(), "凭据用掉即清");
}

/// 场景六 b：收尾时校验只跑到一半（文件对不上）→ 按设置删掉刚加的种子（保留文件）→ 失败。
#[test]
fn e2e_verifying_collect_rollback() {
    let dir = temp_dir("verify-fail");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let progress = Arc::new(Mutex::new((0.0f64, "checkingFiles".to_string())));
    let qb = qb_server_ex(hash, Vec::new(), Some(progress.clone()), None);
    let pter = nexus_site_server("hit", t.clone());
    let hdsky = nexus_site_server("miss", t.clone());
    let mut cfg = test_config(qb.port, pter.port, hdsky.port, &dir);
    cfg.coverage.verify_wait_sec = 2;
    cfg.coverage.verify_poll_sec = 1;

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let _ = run_round(
        &cfg,
        qb_client.clone(),
        &ads,
        &mut index,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        index.contents.values().next().unwrap().coverage["pter"].status,
        CoverageStatus::Verifying
    );

    // 第二轮：校验停在一半且不再 checking → 删种（保留文件）→ Failed
    *progress.lock().unwrap() = (0.5, "pausedUP".to_string());
    let site_before =
        pter.requests("GET /torrents.php").len() + hdsky.requests("GET /torrents.php").len();
    let mut index2 = ContentIndex::load(&cfg.coverage_state_path());
    let report2 = run_round(
        &cfg,
        qb_client,
        &ads,
        &mut index2,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();
    let site_after =
        pter.requests("GET /torrents.php").len() + hdsky.requests("GET /torrents.php").len();
    assert_eq!(site_after, site_before, "收尾不花站点请求");
    assert_eq!(report2.by_status[&CoverageStatus::Failed], 1);
    assert_eq!(
        qb.requests("POST /api/v2/torrents/delete").len(),
        1,
        "按设置删掉刚加的种子"
    );
    let e3 = index2.contents.values().next().unwrap();
    let note = e3.coverage["pter"].note.as_deref().unwrap();
    assert!(note.contains("已删掉"), "note = {note}");
    assert!(e3.coverage["pter"].verify_hash.is_none());
}

/// 场景七：检索预算按「真实请求」记账（一次站内检索要发多条查询词），
/// 预算在半路用完的格子不写 Miss（那是查过才有的负缓存）——本轮跳过、不落格子，
/// 下一轮预算回来从头查，该是什么结论就是什么结论。
#[test]
fn e2e_budget_exhaustion_not_cached() {
    let dir = temp_dir("budget");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server(hash, Vec::new());
    let pter = nexus_site_server("miss", t.clone());
    let hdsky = nexus_site_server("miss", t.clone());
    let mut cfg = test_config(qb.port, pter.port, hdsky.port, &dir);
    // 只给一发请求：严格检索的第一条查询词能出去，第二条就被预算挡下
    cfg.coverage.search_budget = 1;
    cfg.coverage.relaxed_retry_search = false;

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    let report = run_round(
        &cfg,
        qb_client.clone(),
        &ads,
        &mut index,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();

    // 预算一发：一条查询词出门就耗完，检索没查完 → 跳过、不落任何结论
    assert_eq!(
        report.skipped_no_budget, 2,
        "预算一发：两个站都查不完，各记一次「没排上」"
    );
    assert_eq!(report.preskipped, 0, "预算耗尽不是「预筛跳过」");
    assert_eq!(report.by_status.len(), 0);
    let entry = index.contents.values().next().unwrap();
    assert!(entry.coverage.is_empty(), "没查完的格子不写索引");

    // 第二轮预算给足：两个站都查成 → 站内没有 + 源站也没有 → 待找源
    cfg.coverage.search_budget = 40;
    let mut index2 = ContentIndex::load(&cfg.coverage_state_path());
    let report2 = run_round(
        &cfg,
        qb_client,
        &ads,
        &mut index2,
        |_| {},
        RoundOptions {
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report2.by_status[&CoverageStatus::WaitSource], 2);
    let e2 = index2.contents.values().next().unwrap();
    assert_eq!(e2.coverage.len(), 2, "两格都有了");
}

/// 场景八：cookie 失效——站点把检索 302 到登录页。必须记 failed（给人看的原因），
/// 绝不能解析不出结果就当「站内没有」记 miss 负缓存；源种侧同样不能被投毒。
#[test]
fn e2e_login_wall_is_failed_not_miss() {
    let dir = temp_dir("loginwall");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server(hash, Vec::new());
    // pter：一切站点请求都 302 到 /login.php（cookie 失效的典型表现）
    let pter = MockServer::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/login.php") {
            return (
                200,
                "text/html",
                b"<html><body>please login</body></html>".to_vec(),
            );
        }
        (302, "text/plain", b"/login.php".to_vec())
    }));
    let hdsky = nexus_site_server("miss", t.clone());
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
        entry.coverage["pter"].status,
        CoverageStatus::Failed,
        "登录墙是错误，不是「站内没有」"
    );
    let note = entry.coverage["pter"].note.as_deref().unwrap();
    assert!(note.contains("登录"), "note = {note}");
    // 源种侧：pter 报错 → 定位没问成（不记负缓存）；hdsky 站内没有但源没找成 → 同样 failed
    assert_eq!(entry.coverage["hdsky"].status, CoverageStatus::Failed);
    assert!(
        entry.source.is_none(),
        "源种负缓存不能落（pter 的报错不是 clean miss）"
    );
    assert_eq!(report.by_status[&CoverageStatus::Failed], 2);
}

/// 场景九：站点限流（429）——记 failed 给人看、该站降速。
/// 降速的数学在 throttle/resolver 单测里钉，这里钉编排层把 429 正确映射成错误。
#[test]
fn e2e_rate_limited_maps_to_failed() {
    let dir = temp_dir("ratelimit");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server(hash, Vec::new());
    let pter = MockServer::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/torrents.php") {
            return (429, "text/plain", b"Too Many Requests".to_vec());
        }
        (404, "text/plain", Vec::new())
    }));
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
    assert_eq!(
        entry.coverage["pter"].status,
        CoverageStatus::Failed,
        "限流是错误，不是「站内没有」"
    );
    let note = entry.coverage["pter"].note.as_deref().unwrap();
    assert!(note.contains("429"), "note = {note}");
    // 源种侧同样不落负缓存
    assert!(entry.source.is_none());
}

/// 场景十：qB 5.3 实机形态——`/torrents/info` 只有 `tracker`（当前那条 announce），
/// 没有 `tracker_hosts` / `trackers`；`/app/version` 回纯文本 `v5.3.0`。
/// 这两条都是实机踩到的：前者不兜底会让「已在做种」预筛全失效，后者会让连下载器直接失败。
#[test]
fn e2e_single_tracker_field_and_plain_version() {
    let dir = temp_dir("tracker-field");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server_single_tracker(hash, "http://localhost:9091/announce");
    let pter = nexus_site_server("miss", t.clone());
    let hdsky = nexus_site_server("hit", t.clone());
    let cfg = test_config(qb.port, pter.port, hdsky.port, &dir);
    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());

    // 连下载器：5.x 的纯文本版本号也要认
    let hello = qb_client.test_connection().expect("能连上");
    assert!(hello.contains("v5.3.0"), "hello = {hello}");

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
        entry.coverage["hdsky"].status,
        CoverageStatus::Seeding,
        "只有 tracker 字段也要认出「已在做种」"
    );
    // 预筛生效的硬证据：hdsky 站内其实有这条种子，若预筛失效会被当目标站 A 级推送。
    // （hdsky 上仍会有请求——那是 pter 的转种臂把它当源站检索了一轮，属正常）
    assert_eq!(
        qb.requests("POST /api/v2/torrents/add").len(),
        0,
        "已做种的格子绝不推送"
    );
    assert_eq!(
        entry.coverage["pter"].status,
        CoverageStatus::ReseedDraft,
        "另一站照常走转种草稿"
    );
}

/// 场景十一：站点限流页（HTTP 200 + 「请求过于频繁，限制访问间隔 60 秒/次」）。
/// 实机 hdsky 就是这个形态。必须记 failed（并让该站降速），绝不能记成「站内没有」。
#[test]
fn e2e_block_page_is_failed_not_miss() {
    let dir = temp_dir("blockpage");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server(hash, Vec::new());
    let pter = MockServer::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/torrents.php") {
            let page = "<html><head><title>403</title></head>\
                        <body><h3>请求过于频繁，限制访问间隔 60 秒/次</h3></body></html>";
            return (200, "text/html", page.as_bytes().to_vec());
        }
        (404, "text/plain", Vec::new())
    }));
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
    assert_eq!(
        entry.coverage["pter"].status,
        CoverageStatus::Failed,
        "限流页是错误，不是「站内没有」"
    );
    let note = entry.coverage["pter"].note.as_deref().unwrap();
    assert!(note.contains("限流"), "note = {note}");
}

/// 场景十二：cookie 失效的老版 NexusPHP 形态——302 到 `index.php?returnto=…`。
/// 实机 pter 就是这样；只看 URL 里的 login/signin 会漏判，把整站投毒成「站内没有」。
#[test]
fn e2e_returnto_redirect_is_login_wall() {
    let dir = temp_dir("returnto");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server(hash, Vec::new());
    let pter = MockServer::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/torrents.php") {
            return (
                302,
                "text/plain",
                b"/index.php?returnto=torrents.php".to_vec(),
            );
        }
        // 落地页是首页（登录后的入口），不是 login.php
        if path.starts_with("/index.php") {
            return (
                200,
                "text/html",
                b"<html><title>PT</title><body>welcome</body></html>".to_vec(),
            );
        }
        (404, "text/plain", Vec::new())
    }));
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
    assert_eq!(entry.coverage["pter"].status, CoverageStatus::Failed);
    let note = entry.coverage["pter"].note.as_deref().unwrap();
    assert!(
        note.contains("跳") && note.contains("cookie"),
        "note 要说清是跳转（并且两种成因都提）：{note}"
    );
}

/// qB 5.x 形态的下载器 mock：start/stop 回 200，resume/pause 回 404（旧名已删）
fn qb_server_qb5(torrent_hash: String) -> MockServer {
    let torrent_hash = Arc::new(torrent_hash);
    MockServer::start(Arc::new(move |_method, path, _body| {
        let h = &*torrent_hash;
        if path.starts_with("/api/v2/auth/login") {
            return (200, "text/plain", b"Ok.".to_vec());
        }
        if path.starts_with("/api/v2/app/version") {
            return (200, "text/plain", b"v5.3.0".to_vec());
        }
        if path.starts_with("/api/v2/torrents/info") {
            if path.contains("hashes=") {
                let json = format!(
                    r#"[{{"hash":"{h}","progress":1.0,"state":"pausedUP","up_limit":-1}}]"#
                );
                return (200, "application/json", json.into_bytes());
            }
            let json = format!(
                r#"[{{"hash":"{h}","infohash_v2":"","name":"{NAME}","save_path":"/data/","category":"","tags":"","size":105,"progress":1.0,"tracker_hosts":["localhost"]}}]"#
            );
            return (200, "application/json", json.into_bytes());
        }
        if path.starts_with("/api/v2/torrents/files") {
            let json = r#"[{"name":"Pack/a.mkv","size":100,"index":0},{"name":"Pack/s/ch.srt","size":5,"index":1}]"#;
            return (200, "application/json", json.as_bytes().to_vec());
        }
        if path.starts_with("/api/v2/torrents/resume") || path.starts_with("/api/v2/torrents/pause")
        {
            return (404, "text/plain", Vec::new());
        }
        (200, "text/plain", Vec::new())
    }))
}

/// 场景十三：qB 5.x 的启停端点（start/stop，旧的 resume/pause 已 404）。
/// 实机 qB 5.3：只用旧名会静默失败——校验完的种子永远停着不作种、停止态的种子
/// 永远不会开始校验（这正是真实辅种第一跑失败的根因）。
#[test]
fn e2e_qb5_start_stop_endpoints() {
    let dir = temp_dir("qb5-start");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    // qB 5.x 形态：start/stop 回 200，resume/pause 回 404
    let qb = qb_server_qb5(hash);
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
            persist: true,
            ..Default::default()
        },
    )
    .unwrap();

    let entry = index.contents.values().next().unwrap();
    assert_eq!(entry.coverage["pter"].status, CoverageStatus::Assisted);
    assert_eq!(
        qb.requests("POST /api/v2/torrents/start").len(),
        1,
        "qB 5.x 用 start 触发校验"
    );
    assert_eq!(
        qb.requests("POST /api/v2/torrents/resume").len(),
        0,
        "start 成功就不再试旧名"
    );
}

/// 场景：站点种子与本地文件一致、只有根目录名不同（matcher::layout = RootDiffers）——
/// 推种应按 contentLayout=NoSubfolder 复用既有内容目录，而不是拦成「待人工」。
#[test]
fn e2e_root_differs_remaps_with_no_subfolder() {
    let dir = temp_dir("rootdiff");
    // 站点种子根名 Other，本地（qB）是 Pack，内层文件一致
    let t_site = Arc::new(torrent_bytes("Other", FILES));
    let hash = bencode::info_hash_v1(&torrent_bytes("Pack", FILES)).unwrap();
    let captured: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_ex(hash, Vec::new(), None, Some(captured.clone()));
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
        CoverageStatus::Assisted,
        "根名不同但内层一致：应复用目录推种（note: {:?}）",
        entry.coverage["pter"].note
    );
    let reqs = captured.lock().unwrap();
    let body = reqs
        .iter()
        .map(|(_, b)| String::from_utf8_lossy(b).to_string())
        .collect::<Vec<_>>()
        .join("\n---\n");
    assert!(body.contains("name=\"savepath\""), "{body}");
    assert!(
        body.contains("/data/Pack"),
        "savepath 应指到既有内容目录：{body}"
    );
    assert!(body.contains("contentLayout"), "{body}");
    assert!(body.contains("NoSubfolder"), "{body}");
}

/// 场景：站点支持 /api/pieces-hash —— 预扫一次命中后按 id 直下种子推种，
/// 不再走名字检索（/torrents.php 应为零请求）。
#[test]
fn e2e_pieces_hash_fast_path_skips_search() {
    let dir = temp_dir("pieces");
    let local = torrent_bytes("Pack", FILES);
    let hash = bencode::info_hash_v1(&local).unwrap();
    let ph = bencode::pieces_hash(&local).unwrap();
    let t_site = Arc::new(local.clone());
    let ph_c = ph.clone();
    let site = MockServer::start(Arc::new(move |_m, path, _b| {
        if path.starts_with("/api/pieces-hash") {
            let json = format!(r#"{{"ret":0,"data":{{"{ph_c}":1002}}}}"#);
            return (200, "application/json", json.into_bytes());
        }
        if path.starts_with("/download.php") {
            return (200, "application/x-bittorrent", t_site.as_ref().clone());
        }
        if path.starts_with("/torrents.php") {
            return (200, "text/html", b"<p>no</p>".to_vec());
        }
        (404, "text/plain", Vec::new())
    }));
    let qb = qb_server(hash.clone(), Vec::new());
    let cfg = test_config(qb.port, site.port, site.port, &dir);
    // 预置 pieces 缓存与 passkey（免走 qB 导出 / tracker 采集）
    let cache = format!(
        r#"{{"passkeys":{{"127.0.0.1":"pk","localhost":"pk"}},"pieces":{{"{hash}":"{ph}"}}}}"#
    );
    std::fs::write(dir.join("pieces-cache.json"), cache).unwrap();

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
        CoverageStatus::Assisted,
        "{:?}",
        entry.coverage["pter"]
    );
    assert_eq!(
        site.requests("GET /torrents.php").len(),
        0,
        "快速通道命中后不应再走名字检索"
    );
    assert!(
        !site.requests("POST /api/pieces-hash").is_empty(),
        "应走 pieces-hash 预扫"
    );
}

/// BUG-H1 回归：配了上传限速但 qB 未生效（mock 单查回 up_limit=-1）时 verify 带 note，
/// 但这是「本次新加」不是「早已存在」——必须继续 start + 校验到 100%，
/// 不能见 note 就静默落 Assisted 终态（否则种子永远暂停不作种）。
#[test]
fn e2e_assist_new_add_with_limit_note_still_verifies() {
    let dir = temp_dir("assist-limit-note");
    let t_same = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t_same).unwrap();

    let qb = qb_server(hash.clone(), Vec::new());
    let pter = nexus_site_server("hit", t_same.clone());
    let hdsky = nexus_site_server("miss", t_same.clone());
    let mut cfg = test_config(qb.port, pter.port, hdsky.port, &dir);
    cfg.downloader.default_up_limit = 1024;

    let qb_client = Arc::new(QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap());
    let ads = adapters(&cfg);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    run_round(
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
    let cell = &entry.coverage["pter"];
    assert_eq!(cell.status, CoverageStatus::Assisted, "{cell:?}");
    let note = cell.note.as_deref().unwrap_or("");
    assert!(
        note.contains("校验到 100%"),
        "新加种必须走校验流程而不是 note 捷径：{note}"
    );
    assert!(
        note.contains("限速未生效"),
        "限速 note 应并入校验结论备注：{note}"
    );
}

/// BUG-M4 回归：源种定位预算耗尽不等于「站内没有」——
/// 不能落 Miss（那是查过才有的 7 天负缓存），本轮跳过、格子留白等下轮重试。
#[test]
fn e2e_source_budget_exhaustion_writes_no_cell() {
    let dir = temp_dir("source-budget");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let qb = qb_server(hash.clone(), Vec::new());
    let pter = nexus_site_server("miss", t.clone());
    let hdsky = nexus_site_server("miss", t.clone());
    let mut cfg = test_config(qb.port, pter.port, hdsky.port, &dir);
    cfg.coverage.search_budget = 40;
    cfg.coverage.source_budget = 0;

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
    assert!(
        entry.coverage.is_empty(),
        "源种预算耗尽不落任何结论：{:?}",
        entry.coverage
    );
    assert_eq!(
        report.skipped_no_budget, 2,
        "两个站都因预算耗尽跳过（不是预筛跳过）"
    );
}

// ---------- 转种判重命中：目标站已有同分片条目 ----------

/// 目标站（NexusPHP）最小页面：检索结果 + 详情页 + 种子下载。
/// 检索结果带 title 属性（IMDb 判重只认 `<a title=... href=details.php?id=N>`）。
fn reseed_site_server(torrent: Arc<Vec<u8>>) -> MockServer {
    MockServer::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/torrents.php") {
            let html = format!(r#"<a title="{NAME}" href="details.php?id=1002">{NAME}</a>"#);
            return (200, "text/html", html.into_bytes());
        }
        if path.starts_with("/details.php") {
            let html = format!(
                r#"<html><body><h1 id="top">{NAME}</h1>
<div id="kdescr">简介 https://www.imdb.com/title/tt1234567/</div>
<pre>General
Unique ID : 1
Format : Matroska
File size : 100 MiB
Bit rate : 1000 kb/s

Video
Width : 1920 pixels
Height : 1080 pixels</pre>
<a href="download.php?id=1002">下载种子</a></body></html>"#
            );
            return (200, "text/html", html.into_bytes());
        }
        if path.starts_with("/download.php") {
            return (200, "application/x-bittorrent", torrent.as_ref().clone());
        }
        (404, "text/plain", Vec::new())
    }))
}

/// qB mock：本地成员 hash 已完成、可导出源种；目标种 add 后单查直接 100%。
/// `target_present` 控制目标种是否已在下载器（同 hash 策略用例置 true）。
fn qb_server_for_reseed(
    local_hash: String,
    local_torrent: Vec<u8>,
    target_hash: String,
    target_present: bool,
    captured: CapturedRequests,
) -> MockServer {
    let local_hash = Arc::new(local_hash);
    let target_hash = Arc::new(target_hash);
    let local_torrent = Arc::new(local_torrent);
    let present = Arc::new(AtomicBool::new(target_present));
    let added_hash: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let added_for_add = added_hash.clone();
    MockServer::start(Arc::new(move |method, path, body| {
        let lh = &*local_hash;
        let th = &*target_hash;
        if path.starts_with("/api/v2/auth/login") {
            return (200, "text/plain", b"Ok.".to_vec());
        }
        if path.starts_with("/api/v2/app/version") {
            return (200, "text/plain", b"v5.3.0".to_vec());
        }
        if path.starts_with("/api/v2/torrents/info") {
            if path.contains("hashes=") {
                let new_hash = added_hash.lock().unwrap().clone();
                if let Some(ref nh) = new_hash {
                    if path.contains(nh.as_str()) {
                        let json = format!(
                            r#"[{{"hash":"{nh}","progress":1.0,"state":"pausedUP","up_limit":-1,"name":"Pack","save_path":"/data/"}}]"#
                        );
                        return (200, "application/json", json.into_bytes());
                    }
                }
                if path.contains(th.as_str()) && !present.load(Ordering::SeqCst) {
                    return (200, "application/json", b"[]".to_vec());
                }
                if path.contains(th.as_str()) {
                    let json = format!(
                        r#"[{{"hash":"{th}","progress":1.0,"state":"pausedUP","up_limit":-1,"name":"Pack","save_path":"/data/"}}]"#
                    );
                    return (200, "application/json", json.into_bytes());
                }
                if path.contains(lh.as_str()) {
                    let json = format!(
                        r#"[{{"hash":"{lh}","progress":1.0,"state":"pausedUP","up_limit":-1,"name":"Pack","save_path":"/data/"}}]"#
                    );
                    return (200, "application/json", json.into_bytes());
                }
                return (200, "application/json", b"[]".to_vec());
            }
            let json = format!(
                r#"[{{"hash":"{lh}","infohash_v2":"","name":"Pack","save_path":"/data/","category":"","tags":"","size":105,"progress":1.0,"tracker":"http://localhost/announce?passkey=ZZZ","tracker_hosts":[]}}]"#
            );
            return (200, "application/json", json.into_bytes());
        }
        if path.starts_with("/api/v2/torrents/trackers") {
            return (
                200,
                "application/json",
                br#"[{"url":"http://localhost/announce?passkey=ZZZ"}]"#.to_vec(),
            );
        }
        if path.starts_with("/api/v2/torrents/export") {
            return (
                200,
                "application/x-bittorrent",
                local_torrent.as_ref().clone(),
            );
        }
        if path.starts_with("/api/v2/torrents/files") {
            let json = r#"[{"name":"Pack/a.mkv","size":100,"index":0},{"name":"Pack/s/ch.srt","size":5,"index":1}]"#;
            return (200, "application/json", json.as_bytes().to_vec());
        }
        if path.starts_with("/api/v2/torrents/add") {
            assert_eq!(method, "POST");
            captured
                .lock()
                .unwrap()
                .push((path.to_string(), body.to_vec()));
            if let Some(file) = multipart_file(body) {
                *added_for_add.lock().unwrap() = bencode::info_hash_v1(&file);
            }
            present.store(true, Ordering::SeqCst);
            return (200, "text/plain", b"Ok.".to_vec());
        }
        (200, "text/plain", Vec::new())
    }))
}

/// 目标站判重命中（同分片）时，必须下载目标种并推进下载器：
/// 只写 Assisted 不推种，qB 里就永远没有目标站 tracker（「发了 20 个只辅了几个」的根因）。
#[test]
fn e2e_duplicate_hit_pushes_target_torrent_to_downloader() {
    let dir = temp_dir("dup-push");
    // 同文件、同 pieces，根名不同 → info hash 不同，走「新加种 + NoSubfolder 复用目录」
    let t_src = Arc::new(torrent_bytes("Pack", FILES));
    let t_tgt = Arc::new(torrent_bytes("Pack2", FILES));
    let local_hash = bencode::info_hash_v1(&t_src).unwrap();
    let target_hash = bencode::info_hash_v1(&t_tgt).unwrap();
    assert_ne!(local_hash, target_hash, "根名不同，info hash 必须不同");

    let captured: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_for_reseed(
        local_hash.clone(),
        t_src.as_ref().clone(),
        target_hash.clone(),
        false,
        captured.clone(),
    );
    let src = reseed_site_server(t_src.clone());
    let tgt = reseed_site_server(t_tgt.clone());
    let mut cfg = test_config(qb.port, src.port, tgt.port, &dir);
    cfg.coverage.reseed_publish_enabled = true;

    let key = "m:duppush000000000000";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: NAME.into(),
            total_size: 105,
            file_count: FILES.len(),
            manifest_digest: "0".repeat(16),
            torrents: std::collections::BTreeMap::from([(
                local_hash.clone(),
                TorrentMeta {
                    save_path: "/data".into(),
                    category: None,
                    last_seen: 0,
                    tracker_hosts: Vec::new(),
                    hash_v2: None,
                    added_on: 0,
                },
            )]),
            tracker_hosts: Vec::new(),
            first_seen: 0,
            gone_since: None,
            coverage: std::collections::BTreeMap::from([(
                "hdsky".to_string(),
                SiteCoverage {
                    status: CoverageStatus::ReseedDraft,
                    at: 0,
                    url: None,
                    note: None,
                    verify_hash: None,
                },
            )]),
            source: Some(SourceRecord {
                site_id: "pter".into(),
                detail_url: src_url.clone(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
    index.save().unwrap();

    let qb_client = QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap();
    let req = ReseedRequest {
        content_key: key.into(),
        source_site: "pter".into(),
        source_url: src_url,
        target_site: "hdsky".into(),
        name: NAME.into(),
    };
    let mut logs = Vec::new();
    let outcome = run_reseed(
        &cfg,
        &qb_client,
        &req,
        &mut index,
        &mut |l| logs.push(l.to_string()),
        None,
    )
    .unwrap();

    match outcome {
        ReseedOutcome::Duplicate(d) => {
            assert_eq!(d.status, CoverageStatus::Assisted, "{d:?}");
            assert!(
                d.note.contains("下载目标种") || d.note.contains("做种"),
                "判重命中也要下载目标种并做种：{}",
                d.note
            );
        }
        other => panic!("判重命中应返回 Duplicate，实际 {other:?}"),
    }
    let cell = &index.contents[key].coverage["hdsky"];
    assert_eq!(
        cell.status,
        CoverageStatus::Reseeded,
        "目标种必须真的推进 qB 并校验 100%：{cell:?}"
    );
    let adds = captured.lock().unwrap();
    assert_eq!(adds.len(), 1, "必须调用一次 torrents/add");
    let body = String::from_utf8_lossy(&adds[0].1);
    assert!(body.contains("name=\"torrents\""), "add 必须带种子文件");
    assert!(
        body.contains("NoSubfolder"),
        "根名不同应带 contentLayout=NoSubfolder 复用既有目录：{body}"
    );
    assert!(
        logs.iter().any(|l| l.contains("100%")),
        "应走完校验到 100%：{logs:?}"
    );
    let action_log = std::fs::read_to_string(dir.join("reseed-log.jsonl")).unwrap();
    assert!(
        action_log.contains("\"seeded\":true"),
        "动作日志必须记录已辅到 qB：{action_log}"
    );
}

/// 同 hash：qB 不允许同 hash 两条，按策略绝不碰原种子（不补挂 tracker、不打标签），
/// 连 add 请求都不应发出，格子记「同 hash 跳过」。
#[test]
fn e2e_same_hash_skips_without_touching_existing_torrent() {
    let dir = temp_dir("same-hash");
    let t = Arc::new(torrent_bytes("Pack", FILES));
    let hash = bencode::info_hash_v1(&t).unwrap();

    let captured: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_for_reseed(
        hash.clone(),
        t.as_ref().clone(),
        hash.clone(),
        true,
        captured.clone(),
    );
    let src = reseed_site_server(t.clone());
    let tgt = reseed_site_server(t.clone());
    let mut cfg = test_config(qb.port, src.port, tgt.port, &dir);
    cfg.coverage.reseed_publish_enabled = true;
    // 这条用例要覆盖“同 hash 但原种挂的是别站 tracker”的缺口：mock qB 只回
    // localhost，而 hdsky 正常认 localhost；把目标站 host 换掉，避免误判成已做种。
    cfg.sites[1].announce_hosts = vec!["hdsky.example".into()];

    let key = "m:samehash000000000000";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: NAME.into(),
            total_size: 105,
            file_count: FILES.len(),
            manifest_digest: "0".repeat(16),
            torrents: std::collections::BTreeMap::from([(
                hash.clone(),
                TorrentMeta {
                    save_path: "/data".into(),
                    category: None,
                    last_seen: 0,
                    tracker_hosts: Vec::new(),
                    hash_v2: None,
                    added_on: 0,
                },
            )]),
            tracker_hosts: Vec::new(),
            first_seen: 0,
            gone_since: None,
            coverage: std::collections::BTreeMap::from([(
                "hdsky".to_string(),
                SiteCoverage {
                    status: CoverageStatus::ReseedDraft,
                    at: 0,
                    url: None,
                    note: None,
                    verify_hash: None,
                },
            )]),
            source: Some(SourceRecord {
                site_id: "pter".into(),
                detail_url: src_url.clone(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
    index.save().unwrap();

    let qb_client = QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap();
    let req = ReseedRequest {
        content_key: key.into(),
        source_site: "pter".into(),
        source_url: src_url,
        target_site: "hdsky".into(),
        name: NAME.into(),
    };
    let mut logs = Vec::new();
    let outcome = run_reseed(
        &cfg,
        &qb_client,
        &req,
        &mut index,
        &mut |l| logs.push(l.to_string()),
        None,
    )
    .unwrap();

    match outcome {
        ReseedOutcome::Duplicate(d) => {
            assert_eq!(d.status, CoverageStatus::SameHashSkip, "{d:?}");
            assert!(d.note.contains("同 hash"), "{}", d.note);
        }
        other => panic!("同 hash 应返回 Duplicate(SameHashSkip)，实际 {other:?}"),
    }
    let cell = &index.contents[key].coverage["hdsky"];
    assert_eq!(cell.status, CoverageStatus::SameHashSkip, "{cell:?}");
    assert!(
        captured.lock().unwrap().is_empty(),
        "同 hash 不应调用 torrents/add（更不能补挂 tracker/打标签）"
    );
    assert!(logs.iter().any(|l| l.contains("不改动原种子")), "{logs:?}");
    let action_log = std::fs::read_to_string(dir.join("reseed-log.jsonl")).unwrap();
    assert!(
        action_log.contains("\"seeded\":false") && action_log.contains("same_hash_skipped"),
        "同 hash 跳过必须如实记录未辅到 qB：{action_log}"
    );
}
/// 回包后再能检索到条目——引擎要定位、下载目标种并挂到本地数据上，而不是只记待确认。
fn reseed_site_server_upload_existed(torrent: Arc<Vec<u8>>) -> MockServer {
    let uploaded = Arc::new(AtomicBool::new(false));
    MockServer::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/upload.php") {
            let form = r#"<html><body><form action="takeupload.php" method="post" enctype="multipart/form-data">
<input type="text" name="name" value="">
<input type="text" name="small_descr" value="">
<textarea name="descr"></textarea>
<input type="text" name="technical_info" value="">
<input type="text" name="imdb_url" value="">
<input type="file" name="file">
<input type="submit" value="发布">
</form></body></html>"#;
            return (200, "text/html", form.as_bytes().to_vec());
        }
        if path.starts_with("/takeupload.php") {
            uploaded.store(true, Ordering::SeqCst);
            return (
                200,
                "text/html",
                format!(r#"<p>种子已存在</p><a href="details.php?id=1002">{NAME}</a>"#)
                    .into_bytes(),
            );
        }
        if path.starts_with("/torrents.php") {
            if uploaded.load(Ordering::SeqCst) {
                let html = format!(r#"<a title="{NAME}" href="details.php?id=1002">{NAME}</a>"#);
                return (200, "text/html", html.into_bytes());
            }
            return (200, "text/html", "<p>没有种子</p>".as_bytes().to_vec());
        }
        if path.starts_with("/details.php") {
            let html = format!(
                r#"<html><body><h1 id="top">{NAME}</h1>
<a href="download.php?id=1002">下载种子</a></body></html>"#
            );
            return (200, "text/html", html.into_bytes());
        }
        if path.starts_with("/download.php") {
            return (200, "application/x-bittorrent", torrent.as_ref().clone());
        }
        (404, "text/plain", Vec::new())
    }))
}

/// 从 `takeupload` 的 multipart 请求体里取 `name="file"` 的种子字节。
/// 真实站点下载回来的是刚上传的那份 `.torrent`，mock 也照这个行为走，
/// 否则「发后 hash 必须等于上传构造的新 hash」这条校验会被假站点误伤。
fn multipart_file(body: &[u8]) -> Option<Vec<u8>> {
    let first_line_end = body.windows(2).position(|w| w == b"\r\n")?;
    let boundary = &body[..first_line_end]; // "--boundary"
    let delim = [b"\r\n".as_slice(), boundary].concat();
    let mut rest = body;
    while let Some(pos) = rest
        .windows(delim.len())
        .position(|w| w == delim.as_slice())
    {
        rest = &rest[pos + delim.len()..];
        let part = rest.strip_prefix(b"\r\n").unwrap_or(rest);
        if part.starts_with(b"--") {
            break;
        }
        let Some(hdr_end) = part.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&part[..hdr_end]);
        if headers.contains("name=\"file\"") || headers.contains("name=\"torrents\"") {
            let data = &part[hdr_end + 4..];
            let data = data.strip_suffix(b"\r\n").unwrap_or(data);
            return Some(data.to_vec());
        }
    }
    None
}

/// 站上发布成功，但下载回来的目标种根名与构造名不同（站点按自己的标题规范改名）：
/// 文件清单和分片一致，应正常挂回，不能落 posted_unresolved。
#[test]
fn e2e_posted_root_rename_is_mounted() {
    let dir = temp_dir("posted-root-rename");
    let t_src = Arc::new(torrent_bytes("Pack", FILES));
    let t_tgt = Arc::new(torrent_bytes("Pack2", FILES));
    let local_hash = bencode::info_hash_v1(&t_src).unwrap();
    let target_hash = bencode::info_hash_v1(&t_tgt).unwrap();
    assert_ne!(local_hash, target_hash);
    let captured: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let uploads: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_for_reseed(
        local_hash.clone(),
        t_src.as_ref().clone(),
        target_hash.clone(),
        false,
        captured.clone(),
    );
    let src = reseed_site_server(t_src.clone());
    let tgt = reseed_site_server_upload_result(t_tgt.clone(), uploads.clone(), false, false);
    let mut cfg = test_config(qb.port, src.port, tgt.port, &dir);
    cfg.coverage.reseed_publish_enabled = true;

    let key = "m:postedrootrename000";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: NAME.into(),
            total_size: 105,
            file_count: FILES.len(),
            manifest_digest: "0".repeat(16),
            torrents: std::collections::BTreeMap::from([(
                local_hash.clone(),
                TorrentMeta {
                    save_path: "/data".into(),
                    category: None,
                    last_seen: 0,
                    tracker_hosts: Vec::new(),
                    hash_v2: None,
                    added_on: 0,
                },
            )]),
            tracker_hosts: Vec::new(),
            first_seen: 0,
            gone_since: None,
            coverage: std::collections::BTreeMap::from([(
                "hdsky".to_string(),
                SiteCoverage {
                    status: CoverageStatus::ReseedDraft,
                    at: 0,
                    url: None,
                    note: None,
                    verify_hash: None,
                },
            )]),
            source: Some(SourceRecord {
                site_id: "pter".into(),
                detail_url: src_url.clone(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
    index.save().unwrap();

    let qb_client = QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap();
    let req = ReseedRequest {
        content_key: key.into(),
        source_site: "pter".into(),
        source_url: src_url,
        target_site: "hdsky".into(),
        name: NAME.into(),
    };
    let mut logs = Vec::new();
    let outcome = run_reseed(
        &cfg,
        &qb_client,
        &req,
        &mut index,
        &mut |l| logs.push(l.to_string()),
        None,
    )
    .unwrap();

    match outcome {
        ReseedOutcome::Posted(url) => assert!(url.contains("details.php?id=1002"), "{url}"),
        other => panic!("根名不同但文件清单和分片一致，应正常挂回，实际 {other:?}"),
    }
    let cell = &index.contents[key].coverage["hdsky"];
    assert_eq!(cell.status, CoverageStatus::Reseeded, "{cell:?}");
    assert!(
        captured
            .lock()
            .unwrap()
            .iter()
            .any(|(p, _)| p.contains("torrents/add")),
        "应把目标种推进下载器"
    );
}

/// 目标站 mock：上传表单 → takeupload 成功并给详情页；详情页可下载目标种。
/// `uploads` 记下 takeupload 的 multipart 请求体，供「直发提交同一份字段」断言。
/// 下载端点优先返回刚上传的种子（真实站点行为）；没上传时退回静态目标种。
fn reseed_site_server_upload_result(
    torrent: Arc<Vec<u8>>,
    uploads: CapturedRequests,
    reject: bool,
    serve_uploaded: bool,
) -> MockServer {
    let uploaded: Arc<std::sync::Mutex<Option<Vec<u8>>>> = Arc::new(std::sync::Mutex::new(None));
    let uploaded_for_upload = uploaded.clone();
    MockServer::start(Arc::new(move |_method, path, body| {
        if path.starts_with("/upload.php") {
            let form = r#"<html><body><form action="takeupload.php" method="post" enctype="multipart/form-data">
<input type="text" name="name" value="">
<input type="text" name="small_descr" value="">
<textarea name="descr"></textarea>
<input type="text" name="technical_info" value="">
<input type="text" name="imdb_url" value="">
<input type="file" name="file">
<input type="submit" value="发布">
</form></body></html>"#;
            return (200, "text/html", form.as_bytes().to_vec());
        }
        if path.starts_with("/takeupload.php") {
            uploads
                .lock()
                .unwrap()
                .push((path.to_string(), body.to_vec()));
            if reject {
                let html = r#"<html><body><div id="errorbox">发布失败：标题不符合站点规范</div></body></html>"#;
                return (200, "text/html", html.as_bytes().to_vec());
            }
            if let Some(file) = multipart_file(body) {
                *uploaded_for_upload.lock().unwrap() = Some(file);
            }
            let html = format!(r#"<p>发布成功</p><a href="details.php?id=1002">{NAME}</a>"#);
            return (200, "text/html", html.into_bytes());
        }
        // 上传前判重检索：没有条目（上传走 success/reject 分支，不依赖回包跳转）
        if path.starts_with("/torrents.php") {
            return (200, "text/html", "<p>没有种子</p>".as_bytes().to_vec());
        }
        if path.starts_with("/download.php") {
            let picked = if serve_uploaded {
                uploaded
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| torrent.as_ref().clone())
            } else {
                torrent.as_ref().clone()
            };
            return (200, "application/x-bittorrent", picked);
        }
        if path.starts_with("/details.php") {
            let html = format!(
                r#"<html><body><h1 id="top">{NAME}</h1>
<div id="kdescr">简介 https://www.imdb.com/title/tt1234567/</div>
<a href="download.php?id=1002">下载种子</a></body></html>"#
            );
            return (200, "text/html", html.into_bytes());
        }
        (404, "text/plain", Vec::new())
    }))
}

#[test]
fn e2e_posted_inner_file_mismatch_marks_unresolved_not_failed() {
    let dir = temp_dir("posted-layout-mismatch");
    let t_src = Arc::new(torrent_bytes("Pack", FILES));
    let t_tgt = Arc::new(torrent_bytes("Pack2", FILES_OTHER));
    let local_hash = bencode::info_hash_v1(&t_src).unwrap();
    let target_hash = bencode::info_hash_v1(&t_tgt).unwrap();
    let captured: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let uploads: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_for_reseed(
        local_hash.clone(),
        t_src.as_ref().clone(),
        target_hash.clone(),
        false,
        captured.clone(),
    );
    let src = reseed_site_server(t_src.clone());
    let tgt = reseed_site_server_upload_result(t_tgt.clone(), uploads.clone(), false, false);
    let mut cfg = test_config(qb.port, src.port, tgt.port, &dir);
    cfg.coverage.reseed_publish_enabled = true;

    let key = "m:postedlayoutmismatch";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: NAME.into(),
            total_size: 105,
            file_count: FILES.len(),
            manifest_digest: "0".repeat(16),
            torrents: std::collections::BTreeMap::from([(
                local_hash.clone(),
                TorrentMeta {
                    save_path: "/data".into(),
                    category: None,
                    last_seen: 0,
                    tracker_hosts: Vec::new(),
                    hash_v2: None,
                    added_on: 0,
                },
            )]),
            tracker_hosts: Vec::new(),
            first_seen: 0,
            gone_since: None,
            coverage: std::collections::BTreeMap::from([(
                "hdsky".to_string(),
                SiteCoverage {
                    status: CoverageStatus::ReseedDraft,
                    at: 0,
                    url: None,
                    note: None,
                    verify_hash: None,
                },
            )]),
            source: Some(SourceRecord {
                site_id: "pter".into(),
                detail_url: src_url.clone(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
    index.save().unwrap();

    let qb_client = QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap();
    let req = ReseedRequest {
        content_key: key.into(),
        source_site: "pter".into(),
        source_url: src_url,
        target_site: "hdsky".into(),
        name: NAME.into(),
    };
    let mut logs = Vec::new();
    let outcome = run_reseed(
        &cfg,
        &qb_client,
        &req,
        &mut index,
        &mut |l| logs.push(l.to_string()),
        None,
    )
    .unwrap();

    match outcome {
        ReseedOutcome::Duplicate(d) => {
            assert_eq!(d.status, CoverageStatus::ReviewPending, "{d:?}");
            assert!(d.note.starts_with("posted_unresolved"), "{}", d.note);
            assert!(d.note.contains("文件清单"), "{}", d.note);
        }
        other => panic!("内部文件名不一致应落 posted_unresolved，实际 {other:?}"),
    }
    assert!(
        captured.lock().unwrap().is_empty(),
        "文件清单不一致不应调用 torrents/add"
    );
}

#[test]
fn e2e_reviewed_release_reuses_fields_without_refetching() {
    // BUG-M7：审阅产物直发——提交字段与门禁判定同一份，且不再二次抓源页/上传页。
    let dir = temp_dir("reviewed-release");
    let t_src = Arc::new(torrent_bytes("Pack", FILES));
    let t_tgt = Arc::new(torrent_bytes("Pack2", FILES));
    let local_hash = bencode::info_hash_v1(&t_src).unwrap();
    let target_hash = bencode::info_hash_v1(&t_tgt).unwrap();

    let adds: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let uploads: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_for_reseed(
        local_hash.clone(),
        t_src.as_ref().clone(),
        target_hash.clone(),
        false,
        adds.clone(),
    );
    let src = reseed_site_server(t_src.clone());
    let tgt = reseed_site_server_upload_result(t_tgt.clone(), uploads.clone(), false, true);
    let mut cfg = test_config(qb.port, src.port, tgt.port, &dir);
    cfg.coverage.reseed_publish_enabled = true;

    let key = "m:reviewed0000000000000";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: NAME.into(),
            total_size: 105,
            file_count: FILES.len(),
            manifest_digest: "0".repeat(16),
            torrents: std::collections::BTreeMap::from([(
                local_hash.clone(),
                TorrentMeta {
                    save_path: "/data".into(),
                    category: None,
                    last_seen: 0,
                    tracker_hosts: Vec::new(),
                    hash_v2: None,
                    added_on: 0,
                },
            )]),
            tracker_hosts: Vec::new(),
            first_seen: 0,
            gone_since: None,
            coverage: std::collections::BTreeMap::from([(
                "hdsky".to_string(),
                SiteCoverage {
                    status: CoverageStatus::ReseedDraft,
                    at: 0,
                    url: None,
                    note: None,
                    verify_hash: None,
                },
            )]),
            source: Some(SourceRecord {
                site_id: "pter".into(),
                detail_url: src_url.clone(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
    index.save().unwrap();

    let qb_client = QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap();
    let req = ReseedRequest {
        content_key: key.into(),
        source_site: "pter".into(),
        source_url: src_url,
        target_site: "hdsky".into(),
        name: NAME.into(),
    };
    let mut logs = Vec::new();
    let rep = review_reseed(&cfg, &req, &mut |l| logs.push(l.to_string()), None).unwrap();
    // 审阅：源页要抓两次（detail 页 + 原 HTML），上传表单一次
    assert_eq!(
        src.requests("GET /details.php").len(),
        2,
        "审阅阶段应抓源页（详情+原文）：{:?}",
        src.requests("GET /")
    );
    assert_eq!(
        tgt.requests("GET /upload.php").len(),
        1,
        "审阅阶段应抓一次上传表单"
    );
    let reviewed_fields = rep.prepared.fields.clone();

    let src_before = src.requests("GET /details.php").len();
    let form_before = tgt.requests("GET /upload.php").len();
    let outcome = run_reseed_reviewed(
        &cfg,
        &qb_client,
        &req,
        &mut index,
        &mut |l| logs.push(l.to_string()),
        None,
        rep,
    )
    .unwrap();
    assert!(
        matches!(outcome, ReseedOutcome::Posted(_)),
        "审阅产物应能直接发布：{outcome:?}"
    );
    assert_eq!(
        src.requests("GET /details.php").len(),
        src_before,
        "直发不得二次抓源页：{:?}",
        src.requests("GET /")
    );
    assert_eq!(
        tgt.requests("GET /upload.php").len(),
        form_before,
        "直发不得二次抓上传页：{:?}",
        tgt.requests("GET /")
    );

    // 真正提交的 multipart 必须包含审阅时的那份字段值（门禁看到的就是上传的）
    let posts = uploads.lock().unwrap();
    assert_eq!(posts.len(), 1, "应只有一次 takeupload 提交");
    let body = String::from_utf8_lossy(&posts[0].1);
    for (name, value) in &reviewed_fields {
        if value.trim().is_empty() {
            continue;
        }
        assert!(
            body.contains(value.as_str()),
            "提交体缺少审阅字段 {name}={value}"
        );
    }
    assert!(
        logs.iter()
            .any(|l| l.contains("复用审阅产物：提交字段与门禁判定为同一份")),
        "应明确记录复用审阅产物：{logs:?}"
    );
    let cell = &index.contents[key].coverage["hdsky"];
    assert_eq!(cell.status, CoverageStatus::Reseeded, "{cell:?}");
}

#[test]
fn e2e_site_rejected_upload_marks_failed_and_skips_downloader() {
    // S-16：站点写接口级 blocked（takeupload 返回 errorbox）→ 记 failed，不下载/不推种。
    let dir = temp_dir("site-reject");
    let t_src = Arc::new(torrent_bytes("Pack", FILES));
    let t_tgt = Arc::new(torrent_bytes("Pack2", FILES));
    let local_hash = bencode::info_hash_v1(&t_src).unwrap();
    let target_hash = bencode::info_hash_v1(&t_tgt).unwrap();
    let adds: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let uploads: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_for_reseed(
        local_hash.clone(),
        t_src.as_ref().clone(),
        target_hash.clone(),
        false,
        adds.clone(),
    );
    let src = reseed_site_server(t_src.clone());
    let tgt = reseed_site_server_upload_result(t_tgt.clone(), uploads.clone(), true, true);
    let mut cfg = test_config(qb.port, src.port, tgt.port, &dir);
    cfg.coverage.reseed_publish_enabled = true;

    let key = "m:rejected000000000000";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: NAME.into(),
            total_size: 105,
            file_count: FILES.len(),
            manifest_digest: "0".repeat(16),
            torrents: std::collections::BTreeMap::from([(
                local_hash.clone(),
                TorrentMeta {
                    save_path: "/data".into(),
                    category: None,
                    last_seen: 0,
                    tracker_hosts: Vec::new(),
                    hash_v2: None,
                    added_on: 0,
                },
            )]),
            tracker_hosts: Vec::new(),
            first_seen: 0,
            gone_since: None,
            coverage: std::collections::BTreeMap::from([(
                "hdsky".to_string(),
                SiteCoverage {
                    status: CoverageStatus::ReseedDraft,
                    at: 0,
                    url: None,
                    note: None,
                    verify_hash: None,
                },
            )]),
            source: Some(SourceRecord {
                site_id: "pter".into(),
                detail_url: src_url.clone(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
    index.save().unwrap();

    let qb_client = QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap();
    let req = ReseedRequest {
        content_key: key.into(),
        source_site: "pter".into(),
        source_url: src_url,
        target_site: "hdsky".into(),
        name: NAME.into(),
    };
    let rep = review_reseed(&cfg, &req, &mut |_| {}, None).unwrap();
    let err = run_reseed_reviewed(&cfg, &qb_client, &req, &mut index, &mut |_| {}, None, rep)
        .expect_err("站点 errorbox 必须让发布返回错误，而不是假装成功");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("标题不符合站点规范"),
        "应保留站点 errorbox 原文：{msg}"
    );
    assert_eq!(
        uploads.lock().unwrap().len(),
        1,
        "站点拒收不应同字节重投（非「必填缺失」错误）"
    );
    assert!(
        adds.lock().unwrap().is_empty(),
        "站点拒收后不得把目标种推入下载器：{:?}",
        adds.lock().unwrap()
    );
    // CLI/单发路径把错误抛给调用方；格子由编排层（autopost 审计）落 failed。
    let cell = &index.contents[key].coverage["hdsky"];
    assert_eq!(
        cell.status,
        CoverageStatus::ReseedDraft,
        "直发失败不擅改格子：{cell:?}"
    );
}

#[test]
fn e2e_upload_existed_pushes_target_torrent_to_downloader() {
    let dir = temp_dir("existed-push");
    let t_src = Arc::new(torrent_bytes("Pack", FILES));
    let t_tgt = Arc::new(torrent_bytes("Pack2", FILES));
    let local_hash = bencode::info_hash_v1(&t_src).unwrap();
    let target_hash = bencode::info_hash_v1(&t_tgt).unwrap();

    let captured: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server_for_reseed(
        local_hash.clone(),
        t_src.as_ref().clone(),
        target_hash.clone(),
        false,
        captured.clone(),
    );
    let src = reseed_site_server(t_src.clone());
    let tgt = reseed_site_server_upload_existed(t_tgt.clone());
    let mut cfg = test_config(qb.port, src.port, tgt.port, &dir);
    cfg.coverage.reseed_publish_enabled = true;

    let key = "m:existed0000000000000";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: NAME.into(),
            total_size: 105,
            file_count: FILES.len(),
            manifest_digest: "0".repeat(16),
            torrents: std::collections::BTreeMap::from([(
                local_hash.clone(),
                TorrentMeta {
                    save_path: "/data".into(),
                    category: None,
                    last_seen: 0,
                    tracker_hosts: Vec::new(),
                    hash_v2: None,
                    added_on: 0,
                },
            )]),
            tracker_hosts: Vec::new(),
            first_seen: 0,
            gone_since: None,
            coverage: std::collections::BTreeMap::from([(
                "hdsky".to_string(),
                SiteCoverage {
                    status: CoverageStatus::ReseedDraft,
                    at: 0,
                    url: None,
                    note: None,
                    verify_hash: None,
                },
            )]),
            source: Some(SourceRecord {
                site_id: "pter".into(),
                detail_url: src_url.clone(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
    index.save().unwrap();

    let qb_client = QBittorrent::new(&cfg.downloader.url, "admin", "pass").unwrap();
    let req = ReseedRequest {
        content_key: key.into(),
        source_site: "pter".into(),
        source_url: src_url,
        target_site: "hdsky".into(),
        name: NAME.into(),
    };
    let mut logs = Vec::new();
    let outcome = run_reseed(
        &cfg,
        &qb_client,
        &req,
        &mut index,
        &mut |l| logs.push(l.to_string()),
        None,
    )
    .unwrap();

    match outcome {
        ReseedOutcome::Duplicate(d) => {
            assert_eq!(d.status, CoverageStatus::Assisted, "{d:?}");
            assert!(
                d.note.contains("existed")
                    && (d.note.contains("做种") || d.note.contains("加入下载器")),
                "existed 后必须下载目标种并推进下载器：{}",
                d.note
            );
        }
        other => panic!("existed 应返回 Duplicate，实际 {other:?}"),
    }
    let cell = &index.contents[key].coverage["hdsky"];
    assert_ne!(cell.status, CoverageStatus::ReviewPending, "{cell:?}");
    assert_eq!(cell.status, CoverageStatus::Reseeded, "{cell:?}");
    assert_eq!(
        captured.lock().unwrap().len(),
        1,
        "必须调用一次 torrents/add"
    );
    assert!(
        logs.iter().any(|l| l.contains("existed 条目已定位")),
        "应定位到真实条目：{logs:?}"
    );
}
