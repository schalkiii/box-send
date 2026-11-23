//! Integration tests for the `assist.allowOtherVersion` policy in the reseed arm.
//!
//! Scenario: the target site already hosts a same-name entry whose pieces differ
//! from the local member (other version).
//!
//! * `allowOtherVersion=true`  — the dedupe verdict "other version" does NOT stop
//!   the run; the engine continues into the upload orchestration and posts.
//! * `allowOtherVersion=false` — the run stops at dedupe with an `OtherVersion`
//!   cell; the upload form / upload POST / downloader add are never touched.
//!
//! Coverage depth: the non-reviewed `run_reseed` path is driven end to end against
//! minimal mock source site, target site and qBittorrent. The target torrent is a
//! distinct bencode whose `pieces` placeholder differs, so `matching_local_member`
//! returns `None` (other version) rather than `Some` (same pieces).
//!
//! The mock HTTP server below is a trimmed copy of the fixture in
//! `coverage_e2e.rs`; fixtures are duplicated between test files on purpose.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bsend::bencode;
use bsend::config::{Config, CoverageSettings, DownloaderSettings, SiteConf};
use bsend::index::{
    ContentEntry, ContentIndex, CoverageStatus, SiteCoverage, SourceRecord, TorrentMeta,
};
use bsend::qbittorrent::QBittorrent;
use bsend::reseed::{run_reseed, ReseedOutcome, ReseedRequest};
use bsend::sites::Framework;

// ---------- minimal mock HTTP server ----------

type Handler = Arc<dyn Fn(&str, &str, &[u8]) -> (u16, &'static str, Vec<u8>) + Send + Sync>;

type CapturedRequests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

struct MockSite {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    _handle: JoinHandle<()>,
}

impl MockSite {
    fn start(handler: Handler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock site");
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
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len(),
        reason = match status {
            200 => "OK",
            204 => "No Content",
            302 => "Found",
            404 => "Not Found",
            _ => "Error",
        }
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(&payload);
    let _ = stream.flush();
}

// ---------- bencode test fixtures ----------

const NAME: &str = "Black.Lagoon.S01.1080p.Blu-ray.x264-GROUP";
const FILES: &[(&str, i64)] = &[("a.mkv", 100), ("s/ch.srt", 5)];

fn t_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(format!("{}:", s.len()).as_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn t_int(out: &mut Vec<u8>, i: i64) {
    out.extend_from_slice(format!("i{i}e").as_bytes());
}

/// Build a multi-file .torrent. `pieces_tag` is the 20-byte pieces placeholder;
/// two torrents with different tags produce different `pieces_hash`, which is
/// what the reseed-arm dedupe compares to tell same pieces from other version.
fn torrent_bytes(root: &str, files: &[(&str, i64)], pieces_tag: &str) -> Vec<u8> {
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
    t_str(&mut info, pieces_tag);
    let mut out = Vec::new();
    out.push(b'd');
    t_str(&mut out, "announce");
    t_str(&mut out, "http://tracker.example/announce");
    t_str(&mut out, "info");
    out.push(b'd');
    out.extend_from_slice(&info);
    out.push(b'e');
    out.push(b'e');
    out
}

// ---------- site / downloader mocks ----------

fn qb_server(
    local_hash: Arc<String>,
    local_torrent: Arc<Vec<u8>>,
    target_hash: Arc<String>,
    adds: CapturedRequests,
) -> MockSite {
    let present = Arc::new(AtomicBool::new(false));
    MockSite::start(Arc::new(move |method, path, body| {
        let lh = local_hash.as_ref();
        let th = target_hash.as_ref();
        if path.starts_with("/api/v2/auth/login") {
            return (200, "text/plain", b"Ok.".to_vec());
        }
        if path.starts_with("/api/v2/app/version") {
            return (200, "text/plain", b"v5.3.0".to_vec());
        }
        if path.starts_with("/api/v2/torrents/info") {
            if path.contains("hashes=") {
                if path.contains(th.as_str()) && !present.load(Ordering::SeqCst) {
                    return (200, "application/json", b"[]".to_vec());
                }
                let hash = if path.contains(th.as_str()) { th } else { lh };
                let json = format!(
                    r#"[{{"hash":"{hash}","progress":1.0,"state":"pausedUP","up_limit":-1,"name":"Pack","save_path":"/data/"}}]"#
                );
                return (200, "application/json", json.into_bytes());
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
            adds.lock().unwrap().push((path.to_string(), body.to_vec()));
            present.store(true, Ordering::SeqCst);
            return (200, "text/plain", b"Ok.".to_vec());
        }
        (200, "text/plain", Vec::new())
    }))
}

/// Source site: details page with release metadata + downloadable source torrent.
fn src_server(torrent: Arc<Vec<u8>>) -> MockSite {
    MockSite::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/details.php") {
            let html = format!(
                r#"<html><body><h1 id="top">{NAME}</h1>
<div id="kdescr">简介 https://www.imdb.com/title/tt1234567/</div>
<pre>General
Unique ID : 1
Format : Matroska
File size : 100 MiB

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

/// Target site: IMDb-area search misses; name search hits an other-version entry;
/// details page + downloadable (different pieces) torrent; plain upload form;
/// takeupload returns a success page with the new detail link.
fn tgt_server(torrent: Arc<Vec<u8>>, uploads: CapturedRequests) -> MockSite {
    MockSite::start(Arc::new(move |_method, path, _body| {
        if path.starts_with("/torrents.php") {
            // IMDb-area lookup (search_area=4) must miss; the name search below hits.
            if path.contains("search_area=4") {
                return (200, "text/html", "<p>没有种子</p>".as_bytes().to_vec());
            }
            let html = format!(r#"<a title="{NAME}" href="details.php?id=1002">{NAME}</a>"#);
            return (200, "text/html", html.into_bytes());
        }
        if path.starts_with("/details.php") {
            let html = format!(
                r#"<html><body><h1 id="top">{NAME}</h1>
<div id="kdescr">简介 https://www.imdb.com/title/tt1234567/</div>
<pre>General
Format : Matroska
Width : 1920
Height : 1080</pre>
<a href="download.php?id=1002">下载种子</a></body></html>"#
            );
            return (200, "text/html", html.into_bytes());
        }
        if path.starts_with("/download.php") {
            return (200, "application/x-bittorrent", torrent.as_ref().clone());
        }
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
            uploads.lock().unwrap().push((path.to_string(), Vec::new()));
            let html = format!(r#"<p>发布成功</p><a href="details.php?id=1002">{NAME}</a>"#);
            return (200, "text/html", html.into_bytes());
        }
        (404, "text/plain", Vec::new())
    }))
}

// ---------- fixtures ----------

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bsend-otherver-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn test_config(
    qb_port: u16,
    src_port: u16,
    tgt_port: u16,
    dir: &Path,
    allow_other_version: bool,
) -> Config {
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
                url: format!("http://127.0.0.1:{src_port}/"),
                framework: Framework::NexusPHP,
                search_url: None,
                api_key: None,
                announce_hosts: vec!["127.0.0.1".into()],
                announce_url: Some(format!("http://127.0.0.1:{src_port}/announce?passkey=test")),
            },
            SiteConf {
                id: "hdsky".into(),
                name: "天空".into(),
                url: format!("http://localhost:{tgt_port}/"),
                framework: Framework::NexusPHP,
                search_url: None,
                api_key: None,
                announce_hosts: vec!["localhost".into()],
                announce_url: Some(format!("http://localhost:{tgt_port}/announce?passkey=test")),
            },
        ],
        coverage: CoverageSettings {
            allow_other_version,
            reseed_publish_enabled: true,
            per_site_interval_sec: 0.0,
            jitter_sec: 0.0,
            verify_wait_sec: 3,
            verify_poll_sec: 1,
            rate_limit_floor_sec: 1,
            ..CoverageSettings::default()
        },
        user_agent: "bsend-e2e-test".into(),
        cookies: HashMap::new(),
        cookie_cloud: None,
        skipped_sites: Vec::new(),
    }
}

fn insert_draft(index: &mut ContentIndex, key: &str, local_hash: &str, src_url: &str) {
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: NAME.into(),
            total_size: 105,
            file_count: FILES.len(),
            manifest_digest: "0".repeat(16),
            torrents: BTreeMap::from([(
                local_hash.to_string(),
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
            coverage: BTreeMap::from([(
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
                detail_url: src_url.into(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
}

// ---------- tests ----------

/// `allowOtherVersion=true`: dedupe finds a same-name entry with different
/// pieces (other version), yet the run must continue into the upload
/// orchestration and post the release.
#[test]
fn other_version_continues_past_dedupe_and_posts() {
    let dir = temp_dir("allow");
    let t_a = Arc::new(torrent_bytes("Pack", FILES, &"A".repeat(20)));
    let t_b = Arc::new(torrent_bytes("Pack2", FILES, &"B".repeat(20)));
    let local_hash = bencode::info_hash_v1(&t_a).unwrap();
    let target_hash = bencode::info_hash_v1(&t_b).unwrap();

    let adds: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let uploads: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server(
        Arc::new(local_hash.clone()),
        t_a.clone(),
        Arc::new(target_hash),
        adds.clone(),
    );
    let src = src_server(t_a.clone());
    let tgt = tgt_server(t_b.clone(), uploads.clone());
    let cfg = test_config(qb.port, src.port, tgt.port, &dir, true);

    let key = "m:otherver000000000001";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    insert_draft(&mut index, key, &local_hash, &src_url);
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
    .unwrap_or_else(|e| panic!("run_reseed failed: {e:#}\nlogs: {logs:?}"));

    assert!(
        matches!(outcome, ReseedOutcome::Posted(_)),
        "expected Posted, got {outcome:?}\nlogs: {logs:?}"
    );
    assert!(
        logs.iter().any(|l| l.contains("按策略继续发布")),
        "allowOtherVersion=true must continue past the other-version dedupe: {logs:?}"
    );
    assert_eq!(
        uploads.lock().unwrap().len(),
        1,
        "one takeupload submission expected: {:?}",
        tgt.requests("POST /takeupload.php")
    );
    assert_eq!(
        adds.lock().unwrap().len(),
        1,
        "posted torrent must be pushed to the downloader"
    );
    let cell = &index.contents[key].coverage["hdsky"];
    assert_eq!(cell.status, CoverageStatus::Reseeded, "{cell:?}");
}

/// `allowOtherVersion=false`: the other-version verdict stops the run. No upload
/// form fetch, no upload POST, no downloader add; the cell settles as
/// `OtherVersion`.
#[test]
fn other_version_stops_when_disabled() {
    let dir = temp_dir("deny");
    let t_a = Arc::new(torrent_bytes("Pack", FILES, &"A".repeat(20)));
    let t_b = Arc::new(torrent_bytes("Pack2", FILES, &"B".repeat(20)));
    let local_hash = bencode::info_hash_v1(&t_a).unwrap();
    let target_hash = bencode::info_hash_v1(&t_b).unwrap();

    let adds: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let uploads: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
    let qb = qb_server(
        Arc::new(local_hash.clone()),
        t_a.clone(),
        Arc::new(target_hash),
        adds.clone(),
    );
    let src = src_server(t_a.clone());
    let tgt = tgt_server(t_b.clone(), uploads.clone());
    let cfg = test_config(qb.port, src.port, tgt.port, &dir, false);

    let key = "m:otherver000000000002";
    let src_url = format!("http://127.0.0.1:{}/details.php?id=1002", src.port);
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    insert_draft(&mut index, key, &local_hash, &src_url);
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
    .unwrap_or_else(|e| panic!("run_reseed failed: {e:#}\nlogs: {logs:?}"));

    match outcome {
        ReseedOutcome::Duplicate(d) => {
            assert_eq!(d.status, CoverageStatus::OtherVersion, "{d:?}");
            assert!(
                d.note.contains("pieces 不一致") || d.note.contains("其他版本"),
                "note: {}",
                d.note
            );
        }
        other => panic!("expected Duplicate(OtherVersion), got {other:?}\nlogs: {logs:?}"),
    }
    assert!(
        tgt.requests("GET /upload.php").is_empty(),
        "disabled other-version must not fetch the upload form: {:?}",
        tgt.requests("/")
    );
    assert!(
        uploads.lock().unwrap().is_empty(),
        "disabled other-version must not POST takeupload"
    );
    assert_eq!(
        adds.lock().unwrap().len(),
        0,
        "disabled other-version must not push to the downloader"
    );
    let cell = &index.contents[key].coverage["hdsky"];
    assert_eq!(cell.status, CoverageStatus::OtherVersion, "{cell:?}");
}
