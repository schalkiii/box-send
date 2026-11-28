//! 收件箱只读巡检：打开各站 `messages.php`，列出最近消息的标题、发件人、时间、
//! 已读/未读和粗分类。只 GET 列表页，不点开消息、不发站内信、不改覆盖记录。
//!
//! 这里不把标题关键词当成审核结论：真正的状态仍以详情页审核标记为准。分类只用于
//! 让人先看到“有审核不通过/删种/警告的苗头”，避免批量发种前漏看收件箱。

use std::sync::{mpsc, Arc, Mutex};

use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use url::Url;

use crate::sites::nexusphp::NexusPhp;

/// 消息粗分类。只按列表行文字判断，不读正文，不能替代详情页审核结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    ReviewRejected,
    TorrentRemoved,
    Warning,
    Other,
}

impl InboxKind {
    pub fn label(self) -> &'static str {
        match self {
            InboxKind::ReviewRejected => "审核不通过",
            InboxKind::TorrentRemoved => "删种/断种",
            InboxKind::Warning => "警告/违规",
            InboxKind::Other => "其他",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxMessage {
    pub id: String,
    pub title: String,
    pub sender: String,
    pub at: String,
    pub unread: bool,
    pub kind: InboxKind,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxSiteReport {
    pub site: String,
    /// `ok` 或 `error`。
    pub status: String,
    pub title: String,
    /// 登录态疑似失效时为 true；错误原文仍在 `error` 里。
    pub login_expired: bool,
    pub error: String,
    pub messages: Vec<InboxMessage>,
}

/// 并发扫描一组站点。每个站最多取 `limit` 条消息；只打开 `messages.php`。
pub fn scan_sites(
    adapters: &[Arc<NexusPhp>],
    limit: usize,
    concurrency: usize,
) -> Vec<InboxSiteReport> {
    if adapters.is_empty() {
        return Vec::new();
    }
    let queue = Arc::new(Mutex::new(adapters.to_vec()));
    let (tx, rx) = mpsc::channel();
    let workers = concurrency.clamp(1, 12).min(adapters.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            scope.spawn(move || loop {
                let adapter = {
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.pop()
                };
                let Some(adapter) = adapter else {
                    break;
                };
                if tx.send(scan_one(&adapter, limit)).is_err() {
                    break;
                }
            });
        }
        drop(tx);
    });
    let mut rows: Vec<InboxSiteReport> = rx.iter().collect();
    rows.sort_by(|a, b| a.site.cmp(&b.site));
    rows
}

fn scan_one(adapter: &NexusPhp, limit: usize) -> InboxSiteReport {
    let url = match adapter.base.join("messages.php") {
        Ok(u) => u,
        Err(e) => {
            return InboxSiteReport {
                site: adapter.site_id.clone(),
                status: "error".to_string(),
                title: String::new(),
                login_expired: false,
                error: format!("收件箱地址拼不出来：{e}"),
                messages: Vec::new(),
            }
        }
    };
    match adapter.fetch_html(url.as_str(), adapter.base.as_str()) {
        Ok(html) => InboxSiteReport {
            site: adapter.site_id.clone(),
            status: "ok".to_string(),
            title: page_title(&html),
            login_expired: false,
            error: String::new(),
            messages: parse_messages(&html, adapter.base.as_str(), limit),
        },
        Err(e) => {
            // `{e:#}` 把 anyhow 的上下文链一起带出来：只打 `{e}` 会只剩「请求没通」，
            // 看不到底层是 DNS、超时还是连接被重置。
            let error = format!("{e:#}");
            let login_expired = ["登录", "login", "signin", "returnto="]
                .iter()
                .any(|m| error.to_lowercase().contains(&m.to_lowercase()));
            InboxSiteReport {
                site: adapter.site_id.clone(),
                status: "error".to_string(),
                title: String::new(),
                login_expired,
                error,
                messages: Vec::new(),
            }
        }
    }
}

fn page_title(html: &str) -> String {
    let doc = Html::parse_document(html);
    let Ok(sel) = Selector::parse("title") else {
        return String::new();
    };
    doc.select(&sel)
        .next()
        .map(|e| collapse(e.text().collect::<Vec<_>>().join(" ")))
        .unwrap_or_default()
}

/// 解析 `messages.php` 列表。NexusPHP 的常见行是：
/// 已读图标 / 标题链接 / 发件人 / 时间 / 复选框。发件人取第 3 个 `td`，
/// 时间优先取 `span[title]`，没有再用第 4 个 `td`；不同主题列序不一致时
/// 这两项可能留空，但不拿错列硬填。
pub fn parse_messages(html: &str, base: &str, limit: usize) -> Vec<InboxMessage> {
    let doc = Html::parse_document(html);
    let Ok(link_sel) = Selector::parse("a[href*='viewmessage']") else {
        return Vec::new();
    };
    let Ok(td_sel) = Selector::parse("td") else {
        return Vec::new();
    };
    let Ok(span_sel) = Selector::parse("span[title]") else {
        return Vec::new();
    };
    let Ok(img_sel) = Selector::parse("img") else {
        return Vec::new();
    };
    let Ok(base_url) = Url::parse(base) else {
        return Vec::new();
    };

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for link in doc.select(&link_sel) {
        let href = link.value().attr("href").unwrap_or("").trim();
        if !href.contains("action=viewmessage") {
            continue;
        }
        let title = collapse(link.text().collect::<Vec<_>>().join(" "));
        if title.is_empty() {
            continue;
        }
        let full = base_url.join(href).ok();
        let id = full
            .as_ref()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "id")
                    .map(|(_, v)| v.to_string())
            })
            .or_else(|| {
                href.split("id=")
                    .nth(1)
                    .map(|s| s.split('&').next().unwrap_or(s).to_string())
            })
            .unwrap_or_default();
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        let row = link
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().name() == "tr");
        let cells: Vec<String> = row
            .map(|r| {
                r.select(&td_sel)
                    .map(|td| collapse(td.text().collect::<Vec<_>>().join(" ")))
                    .collect()
            })
            .unwrap_or_default();
        let row_text = row
            .map(|r| collapse(r.text().collect::<Vec<_>>().join(" ")))
            .unwrap_or_else(|| title.clone());
        let sender = cells.get(2).cloned().unwrap_or_default();
        let at = row
            .and_then(|r| {
                r.select(&span_sel)
                    .find_map(|s| s.value().attr("title").map(str::to_string))
            })
            .or_else(|| cells.get(3).cloned())
            .unwrap_or_default();
        let unread = row.is_some_and(|r| {
            r.select(&img_sel).any(|img| {
                let class = img.value().attr("class").unwrap_or("").to_lowercase();
                let alt = img.value().attr("alt").unwrap_or("").to_lowercase();
                let title = img.value().attr("title").unwrap_or("");
                class.contains("unread") || alt.contains("unread") || title.contains("未读")
            })
        });
        let kind = classify(&format!("{title} {row_text}"));
        out.push(InboxMessage {
            id,
            title,
            sender,
            at,
            unread,
            kind,
            url: full
                .map(|u| u.to_string())
                .unwrap_or_else(|| href.to_string()),
        });
        if limit > 0 && out.len() >= limit {
            break;
        }
    }
    out
}

fn classify(text: &str) -> InboxKind {
    let t = text.to_lowercase();
    const REJECT: &[&str] = &[
        "审核不通过",
        "审核未通过",
        "未通过审核",
        "审核拒绝",
        "拒绝原因",
        "驳回",
        "不合格",
    ];
    const REMOVED: &[&str] = &["删种", "删除种子", "已被删除", "断种", "候选超时"];
    const WARNING: &[&str] = &["警告", "违规", "处罚", "申诉", "提醒"];
    if REJECT.iter().any(|m| t.contains(m)) {
        InboxKind::ReviewRejected
    } else if REMOVED.iter().any(|m| t.contains(m)) {
        InboxKind::TorrentRemoved
    } else if WARNING.iter().any(|m| t.contains(m)) {
        InboxKind::Warning
    } else {
        InboxKind::Other
    }
}

fn collapse(s: String) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
<html><head><title>库非 :: 收件箱</title></head><body>
<table>
<tr>
  <td class=rowfollow align=center><img class="unreadpm" src="pic/trans.gif" alt="Unread" title=未读 /></td>
  <td class=rowfollow align=left><a href="messages.php?action=viewmessage&amp;id=42">审核不通过：缺少截图</a></td>
  <td class=rowfollow align=left>管理组</td>
  <td class=rowfollow nowrap><span title="2026-11-28 10:00:00">1小时前</span></td>
  <td class=rowfollow><input type=checkbox name="messages[]" value="42"></td>
</tr>
<tr>
  <td class=rowfollow align=center><img class="readpm" src="pic/trans.gif" alt="Read" title=已读 /></td>
  <td class=rowfollow align=left><a href="messages.php?action=viewmessage&amp;id=41">2026-10 认领结算</a></td>
  <td class=rowfollow align=left>系统</td>
  <td class=rowfollow nowrap><span title="2026-11-01 00:00:23">27天3时前</span></td>
  <td class=rowfollow><input type=checkbox name="messages[]" value="41"></td>
</tr>
</table></body></html>"#;

    #[test]
    fn parses_rows_and_marks_unread() {
        let rows = parse_messages(SAMPLE, "https://example.test/", 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "42");
        assert_eq!(rows[0].sender, "管理组");
        assert_eq!(rows[0].at, "2026-11-28 10:00:00");
        assert!(rows[0].unread);
        assert_eq!(rows[0].kind, InboxKind::ReviewRejected);
        assert_eq!(
            rows[0].url,
            "https://example.test/messages.php?action=viewmessage&id=42"
        );
        assert!(!rows[1].unread);
        assert_eq!(rows[1].kind, InboxKind::Other);
    }

    #[test]
    fn limit_and_duplicate_ids_are_handled() {
        let rows = parse_messages(SAMPLE, "https://example.test/", 1);
        assert_eq!(rows.len(), 1);
        let duplicated = SAMPLE.replace("id=41", "id=42");
        let rows = parse_messages(&duplicated, "https://example.test/", 10);
        assert_eq!(rows.len(), 1, "同一 id 只保留一条");
    }

    #[test]
    fn classifies_removed_and_warning() {
        assert_eq!(classify("候选超时，种子已删除"), InboxKind::TorrentRemoved);
        assert_eq!(classify("管理员警告：简介违规"), InboxKind::Warning);
        assert_eq!(classify("认领结算"), InboxKind::Other);
    }

    #[test]
    fn empty_page_returns_no_messages() {
        assert!(parse_messages(
            "<html><body>没有消息</body></html>",
            "https://example.test/",
            10
        )
        .is_empty());
    }
}
