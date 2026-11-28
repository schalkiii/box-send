//! 按站点统计已发种数量与最后一次发种时间（只读运行日志，不联网）。
//!
//! 数据源是数据目录里的两份追加日志：
//! - `auto-audit.jsonl`（自动发种/手工转种审计）
//! - `reseed-log.jsonl`（转种动作日志）
//!
//! 只算“站点收下了新种”的结果：`posted` / `posted_verifying` / `pending_review` /
//! `posted_unresolved`。`existed_pending`（站上本来就有）和 `skipped_duplicate`
//! 不是新发种，不计入；URL 带 `existed=1` 的历史 `posted` 记录同样不算新发种。
//! 同一站点 + 同一内容键在两份日志里都出现时只算一条。
//! 一个内容被重复发布到同一站（新详情页）目前也只算一条：这是“覆盖了多少内容”，
//! 不是“上传了多少次”。

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

const SUCCESS_RESULTS: [&str; 4] = [
    "posted",
    "posted_verifying",
    "pending_review",
    "posted_unresolved",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SitePostStat {
    pub site: String,
    pub posted: usize,
    /// Unix 秒；0 表示日志里没有可用时间
    pub last_at: i64,
    /// 最后一次成功记录里的详情页链接；日志没给链接时为空
    pub last_url: String,
}

impl SitePostStat {
    /// 北京时间显示（固定 UTC+8；日志里的 at 都是 Unix 秒）。
    pub fn last_at_text(&self) -> String {
        if self.last_at <= 0 {
            String::new()
        } else {
            format_cst(self.last_at)
        }
    }
}

/// 读取两份日志并按站点汇总。文件不存在按空日志处理；坏行跳过，不影响其他行。
pub fn site_post_stats(data_dir: &Path) -> Vec<SitePostStat> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut stats: BTreeMap<String, SitePostStat> = BTreeMap::new();

    for (file, site_field) in [
        ("auto-audit.jsonl", "site"),
        ("reseed-log.jsonl", "target_site"),
    ] {
        for row in read_jsonl(&data_dir.join(file)) {
            let result = row.get("result").and_then(Value::as_str).unwrap_or("");
            if !SUCCESS_RESULTS.contains(&result) {
                continue;
            }
            let url = row.get("url").and_then(Value::as_str).unwrap_or("");
            // 历史版本把站点回包 `existed=1` 也记成 posted；链接指向的是站上原有
            // 同 hash 条目，不是这次新发，不能算“已发种”。
            if url.contains("existed=1") {
                continue;
            }
            let site = row
                .get(site_field)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if site.is_empty() {
                continue;
            }
            let key = row
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            // 没有内容键的旧记录用链接兜底；再没有就按时间单独算，避免把多轮尝试并成一条。
            let ident = if !key.is_empty() {
                key
            } else {
                row.get("url")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("at:{}", unix_seconds(&row)))
            };
            let stat = stats.entry(site.clone()).or_insert_with(|| SitePostStat {
                site: site.clone(),
                posted: 0,
                last_at: 0,
                last_url: String::new(),
            });
            if seen.insert((site.clone(), ident)) {
                stat.posted += 1;
            }
            let at = unix_seconds(&row);
            if at >= stat.last_at {
                stat.last_at = at;
                if let Some(url) = row.get("url").and_then(Value::as_str) {
                    if !url.trim().is_empty() {
                        stat.last_url = url.to_string();
                    }
                }
            }
        }
    }

    let mut rows: Vec<SitePostStat> = stats.into_values().collect();
    rows.sort_by(|a, b| {
        b.posted
            .cmp(&a.posted)
            .then(b.last_at.cmp(&a.last_at))
            .then(a.site.cmp(&b.site))
    });
    rows
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                None
            } else {
                serde_json::from_str(line).ok()
            }
        })
        .collect()
}

fn unix_seconds(row: &Value) -> i64 {
    match row.get("at") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => parse_iso_seconds(s).unwrap_or(0),
        _ => 0,
    }
}

/// 仅支持日志里可能出现的 RFC3339（如 `2026-11-27T19:23:00+08:00`）。
/// 解析失败返回 None，调用方按 0 处理，不影响其他记录。
fn parse_iso_seconds(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let date = raw.get(0..10)?;
    let time = raw.get(11..19)?;
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    let mut tp = time.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mm: i64 = tp.next()?.parse().ok()?;
    let ss: i64 = tp.next()?.parse().ok()?;
    let mut at = days_from_civil(y, m, d) * 86400 + hh * 3600 + mm * 60 + ss;
    // 带时区偏移时换算成 UTC；日志一般用 +08:00，但这里不硬编码。
    if let Some(sign_pos) = raw.get(19..).and_then(|tail| {
        tail.find(['+', '-'])
            .map(|p| p + 19)
            .filter(|_| tail.len() >= 6)
    }) {
        let sign = if raw.as_bytes()[sign_pos] == b'-' {
            -1
        } else {
            1
        };
        let off = raw.get(sign_pos + 1..sign_pos + 6)?;
        let oh: i64 = off.get(0..2)?.parse().ok()?;
        let om: i64 = off.get(3..5)?.parse().ok()?;
        at -= sign * (oh * 3600 + om * 60);
    }
    Some(at)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Unix 秒 → 北京时间 `YYYY-MM-DD HH:MM`（固定 UTC+8）。
pub fn format_cst(at: i64) -> String {
    let shifted = at + 8 * 3600;
    let days = shifted.div_euclid(86400);
    let secs = shifted.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bsend-poststats-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn merges_both_logs_and_does_not_double_count_same_key() {
        let dir = temp_dir("merge");
        fs::write(
            dir.join("auto-audit.jsonl"),
            concat!(
                r#"{"at":100,"site":"a","key":"m:1","result":"posted","url":"https://a/1"}"#,
                "\n",
                r#"{"at":90,"site":"a","key":"m:1","result":"failed"}"#,
                "\n",
                r#"{"at":110,"site":"b","key":"m:2","result":"pending_review"}"#,
            ),
        )
        .unwrap();
        fs::write(
            dir.join("reseed-log.jsonl"),
            concat!(
                r#"{"at":120,"target_site":"a","key":"m:1","result":"pending_review","url":"https://a/1"}"#,
                "\n",
                r#"{"at":130,"target_site":"a","key":"m:3","result":"posted","url":"https://a/3"}"#,
            ),
        )
        .unwrap();
        let rows = site_post_stats(&dir);
        assert_eq!(rows.len(), 2);
        let a = rows.iter().find(|r| r.site == "a").unwrap();
        assert_eq!(a.posted, 2);
        assert_eq!(a.last_at, 130);
        assert_eq!(a.last_url, "https://a/3");
        let b = rows.iter().find(|r| r.site == "b").unwrap();
        assert_eq!(b.posted, 1);
    }

    #[test]
    fn ignores_non_posting_results() {
        let dir = temp_dir("ignore");
        fs::write(
            dir.join("auto-audit.jsonl"),
            concat!(
                r#"{"at":1,"site":"a","key":"m:1","result":"existed_pending"}"#,
                "\n",
                r#"{"at":2,"site":"a","key":"m:2","result":"skipped_duplicate"}"#,
                "\n",
                r#"{"at":3,"site":"a","key":"m:3","result":"blocked"}"#,
            ),
        )
        .unwrap();
        assert!(site_post_stats(&dir).is_empty());
    }

    #[test]
    fn ignores_existed_history_even_if_result_says_posted() {
        let dir = temp_dir("existed");
        fs::write(
            dir.join("auto-audit.jsonl"),
            concat!(
                r#"{"at":10,"site":"a","key":"m:1","result":"posted","url":"https://a/1"}"#,
                "\n",
                r#"{"at":20,"site":"a","key":"m:2","result":"posted","url":"https://a/2&existed=1"}"#,
            ),
        )
        .unwrap();
        let rows = site_post_stats(&dir);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].posted, 1);
        assert_eq!(rows[0].last_at, 10, "existed 记录不能抬高最后发种时间");
        assert_eq!(rows[0].last_url, "https://a/1");
    }

    #[test]
    fn formats_cst() {
        assert_eq!(format_cst(0), "1970-01-01 08:00");
        assert_eq!(format_cst(1795714131), "2026-11-27 01:28");
    }

    #[test]
    fn parses_iso_with_offset() {
        assert_eq!(
            parse_iso_seconds("2026-11-27T19:23:00+08:00"),
            Some(1795778580)
        );
    }
}
