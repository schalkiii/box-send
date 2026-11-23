//! Integration tests for the `assist.skipKeywords` blacklist gate.
//!
//! Covered policy: a reseed draft whose name hits a blacklisted keyword must be
//! skipped before the reviewer / runner run, and must make zero requests to the
//! configured site. The skip emits a blacklist log line and writes no audit
//! record. A paired control case with a non-matching keyword proves the flow
//! otherwise reaches the reviewer.
//!
//! The minimal mock HTTP server below is a trimmed copy of the fixture in
//! `coverage_e2e.rs`; fixtures are duplicated between test files on purpose
//! (integration tests cannot share `#[cfg(test)]` helpers).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::anyhow;

use bsend::autopost::{audit_path_of, run_autopost_with, AutopostEvent, AutopostOptions};
use bsend::config::Config;
use bsend::index::{
    ContentEntry, ContentIndex, CoverageStatus, SiteCoverage, SourceRecord, TorrentMeta,
};
use bsend::reseed::{DuplicateOutcome, ReseedOutcome, ReseedRequest, ReviewReport};
use bsend::throttle::SiteThrottle;

// ---------- minimal mock HTTP server ----------

type Handler = Arc<dyn Fn(&str, &str, &[u8]) -> (u16, &'static str, Vec<u8>) + Send + Sync>;

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

    fn request_count(&self) -> usize {
        self.log.lock().unwrap().len()
    }

    fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
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

// ---------- fixtures ----------

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bsend-skip-kw-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_boxsend_json(dir: &Path, port: u16, keywords: &[&str]) -> PathBuf {
    let kw: String = keywords
        .iter()
        .map(|k| format!("\"{k}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let data_dir = dir.to_string_lossy().replace('\\', "/");
    let json = format!(
        r#"{{
          "dataDir": "{data_dir}",
          "downloader": {{"url": "http://127.0.0.1:1"}},
          "sourceSites": [
            {{"id": "sbpt", "name": "SBPT", "url": "http://127.0.0.1:{port}/", "framework": "NexusPHP", "enabled": true}}
          ],
          "assist": {{
            "autoReseedSites": ["sbpt"],
            "skipKeywords": [{kw}],
            "publishEnabled": true
          }}
        }}"#
    );
    let p = dir.join("boxsend.json");
    std::fs::write(&p, json).unwrap();
    p
}

fn insert_draft(index: &mut ContentIndex, key: &str, name: &str) {
    index.contents.insert(
        key.to_string(),
        ContentEntry {
            name: name.into(),
            total_size: 105,
            file_count: 2,
            manifest_digest: "0".repeat(16),
            torrents: BTreeMap::from([(
                "a".repeat(40),
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
                "sbpt".to_string(),
                SiteCoverage {
                    status: CoverageStatus::ReseedDraft,
                    at: 0,
                    url: None,
                    note: None,
                    verify_hash: None,
                },
            )]),
            source: Some(SourceRecord {
                site_id: "luckpt".into(),
                detail_url: "https://pt.luckpt.de/details.php?id=1".into(),
                resolved_at: 0,
                miss_at: None,
            }),
            forbid_reseed: false,
        },
    );
}

fn collect<'a>(
    logs: &'a mut Vec<String>,
    outcomes: &'a mut Vec<(String, &'static str)>,
) -> impl FnMut(AutopostEvent) + 'a {
    move |e| match e {
        AutopostEvent::Log(m) => logs.push(m),
        AutopostEvent::Outcome { key, result, .. } => outcomes.push((key, result)),
        _ => {}
    }
}

// ---------- tests ----------

/// A draft whose name hits `skipKeywords` is skipped before review: zero site
/// requests, reviewer and runner both untouched, blacklist log emitted, and no
/// audit entry written for the draft.
#[test]
fn skip_keyword_blocks_before_review_with_zero_site_requests() {
    let dir = temp_dir("hit");
    let site = MockSite::start(Arc::new(|_m, _p, _b| (404, "text/plain", Vec::new())));
    let cfg_path = write_boxsend_json(&dir, site.port, &["儿童"]);
    let cfg = Config::load(&cfg_path).unwrap();

    let key = "m:skipkw00000000000001";
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    insert_draft(&mut index, key, "儿童启蒙动画合集.1080p.Blu-ray.x264-GROUP");
    index.save().unwrap();

    let reviewed = Arc::new(AtomicUsize::new(0));
    let ran = Arc::new(AtomicUsize::new(0));
    let r_reviewed = reviewed.clone();
    let r_ran = ran.clone();

    let mut reviewer =
        |_: &Config, _: &ReseedRequest, _: &mut dyn FnMut(&str), _: Option<&SiteThrottle>| {
            r_reviewed.fetch_add(1, Ordering::SeqCst);
            panic!("reviewer must not be reached when a skipKeyword hits");
        };
    let mut runner = |_: &Config,
                      _: &ReseedRequest,
                      _: &mut ContentIndex,
                      _: &mut dyn FnMut(&str),
                      _: Option<&SiteThrottle>,
                      _: Option<ReviewReport>| {
        r_ran.fetch_add(1, Ordering::SeqCst);
        panic!("runner must not be reached when a skipKeyword hits");
    };

    let mut logs = Vec::new();
    let mut outcomes = Vec::new();
    let opts = AutopostOptions {
        dry_run: false,
        limit: None,
        sites: None,
        reconcile: false,
        loop_mode: false,
        interval_secs: 1800,
    };
    let summary = run_autopost_with(
        &cfg,
        &opts,
        None,
        &mut collect(&mut logs, &mut outcomes),
        &mut reviewer,
        &mut runner,
    )
    .unwrap();

    assert_eq!(summary.posted, 0);
    assert_eq!(
        site.request_count(),
        0,
        "blacklisted draft must make zero requests to the configured site: {:?}",
        site.requests()
    );
    assert_eq!(reviewed.load(Ordering::SeqCst), 0, "reviewer untouched");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "runner untouched");
    assert!(
        logs.iter().any(|l| l.contains("命中内容类型黑名单")),
        "blacklist skip log missing: {logs:?}"
    );
    assert!(
        outcomes.is_empty(),
        "a blacklist skip must not emit a draft outcome: {outcomes:?}"
    );

    // Current behavior: the blacklist skip writes no audit record (no
    // review_failed / blocked / failed / posted line for this draft).
    let audit = audit_path_of(&cfg);
    if let Ok(text) = std::fs::read_to_string(&audit) {
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let v: serde_json::Value =
                serde_json::from_str(line).expect("audit line must be valid JSON");
            assert_ne!(
                v.get("key").and_then(|k| k.as_str()),
                Some(key),
                "skip must not write an audit entry: {line}"
            );
        }
    }
}

/// Control case: with a keyword that does not match the draft name, the flow
/// reaches the reviewer. The reviewer stub fails on purpose (review_failed),
/// which proves the gate order is skip → reviewer, not reviewer → skip.
#[test]
fn non_matching_keyword_reaches_reviewer() {
    let dir = temp_dir("control");
    let site = MockSite::start(Arc::new(|_m, _p, _b| (404, "text/plain", Vec::new())));
    let cfg_path = write_boxsend_json(&dir, site.port, &["zzz-no-such-keyword"]);
    let cfg = Config::load(&cfg_path).unwrap();

    let key = "m:skipkw00000000000002";
    let mut index = ContentIndex::load(&cfg.coverage_state_path());
    insert_draft(&mut index, key, "Show.S01.1080p.Blu-ray.x264-GROUP");
    index.save().unwrap();

    let reviewed = Arc::new(AtomicUsize::new(0));
    let ran = Arc::new(AtomicUsize::new(0));
    let r_reviewed = reviewed.clone();
    let r_ran = ran.clone();

    let mut reviewer =
        |_: &Config, req: &ReseedRequest, _: &mut dyn FnMut(&str), _: Option<&SiteThrottle>| {
            r_reviewed.fetch_add(1, Ordering::SeqCst);
            Err::<ReviewReport, _>(anyhow!("stub reviewer failed on purpose: {}", req.name))
        };
    let mut runner = |_: &Config,
                      _: &ReseedRequest,
                      _: &mut ContentIndex,
                      _: &mut dyn FnMut(&str),
                      _: Option<&SiteThrottle>,
                      _: Option<ReviewReport>| {
        r_ran.fetch_add(1, Ordering::SeqCst);
        Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
            status: CoverageStatus::Assisted,
            url: None,
            note: "stub".into(),
        }))
    };

    let mut logs = Vec::new();
    let mut outcomes = Vec::new();
    let opts = AutopostOptions {
        dry_run: false,
        limit: None,
        sites: None,
        reconcile: false,
        loop_mode: false,
        interval_secs: 1800,
    };
    let summary = run_autopost_with(
        &cfg,
        &opts,
        None,
        &mut collect(&mut logs, &mut outcomes),
        &mut reviewer,
        &mut runner,
    )
    .unwrap();

    assert_eq!(
        reviewed.load(Ordering::SeqCst),
        1,
        "non-matching draft must reach the reviewer"
    );
    assert_eq!(
        ran.load(Ordering::SeqCst),
        0,
        "reviewer failure must not reach the runner"
    );
    assert_eq!(summary.posted, 0);
    assert!(
        outcomes
            .iter()
            .any(|(k, r)| k == key && *r == "review_failed"),
        "stub review failure must be recorded as review_failed: {outcomes:?}"
    );
    // The stub itself does no network; the harness must still not have hit the site.
    assert_eq!(site.request_count(), 0);
}
