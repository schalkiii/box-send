//! 发后审阅、详情链接定位与发后自检。

use super::*;

/// 审阅结果：将要提交的全部内容摆出来，不 POST、不落账
pub struct ReviewReport {
    pub info: ReleaseInfo,
    pub preflight: Preflight,
    pub prepared: PreparedUpload,
    /// 站点官方预审（LuckAudit）结论——支持该接口的站才有
    pub audit: Option<AuditVerdict>,
    /// 官方预审没跑通时的原因（只提示，不阻断审阅）
    pub audit_note: Option<String>,
    /// 发布器复用（`run_reseed_reviewed`）；渲染与单测不需要
    pub(crate) carry: Option<ReviewCarry>,
}

impl ReviewReport {
    /// 人看的审阅文本：预检结论 + 官方预审 + 逐字段值（长值截断）
    pub fn render(&self, max: usize) -> String {
        let mut out = String::new();
        out.push_str("==== 转种审阅（只审不发）====\n");
        out.push_str(&format!("提交地址：{}\n", self.prepared.action));
        out.push_str(&format!("预检：{}\n", self.preflight.summary()));
        for e in &self.preflight.errors {
            out.push_str(&format!("  阻断：{e}\n"));
        }
        for w in &self.preflight.warnings {
            out.push_str(&format!("  警告：{w}\n"));
        }
        for f in &self.preflight.fixes {
            out.push_str(&format!("  已自动补：{f}\n"));
        }
        if let Some(w) = &self.prepared.tag_warn {
            out.push_str(&format!("  警告：{w}\n"));
        }
        for n in &self.prepared.select_notes {
            out.push_str(&format!("  选档提示：{n}\n"));
        }
        for g in &self.prepared.required_gaps {
            out.push_str(&format!("  阻断：{g}\n"));
        }
        match (&self.audit, &self.audit_note) {
            (Some(a), _) => {
                out.push_str(&format!(
                    "官方预审（LuckAudit）：{} {}\n",
                    if a.passed { "✅" } else { "❌" },
                    a.summary()
                ));
                for d in &a.details {
                    out.push_str(&format!("  {}\n", d.line()));
                }
                for s in &a.suggestions {
                    out.push_str(&format!("  建议：{s}\n"));
                }
            }
            (None, Some(n)) => out.push_str(&format!("官方预审（LuckAudit）：没跑通（{n}）\n")),
            (None, None) => {}
        }
        if !self.prepared.picked_tags.is_empty() {
            out.push_str(&format!(
                "标签勾选：{}\n",
                self.prepared.picked_tags.join("、")
            ));
        }
        let picks = select_summary(&self.prepared);
        if !picks.is_empty() {
            out.push_str(&format!("选档：{picks}\n"));
        }
        out.push_str("将提交的字段：\n");
        for (n, v) in &self.prepared.fields {
            out.push_str(&format!(
                "  {n} = {}\n",
                truncate_chars(&mask_field_value(n, v), max)
            ));
        }
        out
    }
}

/// 审阅渲染的字段脱敏：按**字段名**判定（不按值猜），命中凭据类一律隐藏，
/// 只保留「有这个字段 + 长度」，审阅仍能确认 passkey 等是否透传成功。
/// 泄漏面：render/dump_fields 会进终端、GUI 日志与反馈截图，passkey 等同账号凭据。
pub(super) fn mask_field_value(name: &str, value: &str) -> String {
    const SECRET_HINTS: [&str; 8] = [
        "passkey", "token", "auth", "cookie", "secure", "uid", "secret", "password",
    ];
    let n = name.to_ascii_lowercase();
    if SECRET_HINTS.iter().any(|h| n.contains(h)) {
        format!("<已隐藏 len={}>", value.len())
    } else {
        value.to_string()
    }
}

/// 长值截断（按字符切，中文不切半个）
pub(super) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…（共 {} 字节）", s.len())
}

/// 审阅一条龙：源页解析 → 预检 → 构造上传字段，但不 POST。
/// 预检不通过也照常返回报告（审阅就是要看清问题），调用方看 `preflight.ok()`。
/// `throttle` 给 autopost 用：源页/上传页/官方预审都是真实站点请求，必须领号；
/// 单条人工审阅传 `None`（没有共享节流器，不额外限速）。
pub fn review_reseed(
    cfg: &Config,
    req: &ReseedRequest,
    log: &mut dyn FnMut(&str),
    throttle: Option<&SiteThrottle>,
) -> Result<ReviewReport> {
    if let Some(t) = throttle {
        if !t.acquire(&req.source_site, BudgetKind::Source) {
            bail!("审阅预算用完（源站定位），本轮跳过该草稿");
        }
    }
    let mut rel = prepare_released(cfg, req, log)?;
    if let Some(t) = throttle {
        if !t.acquire(&req.target_site, BudgetKind::Search) {
            bail!("审阅预算用完（目标站上传页），本轮跳过该草稿");
        }
    }
    log("抓目标站上传表单，构造提交字段…");
    let rules = cfg.coverage.rules_for(&req.target_site);
    let mut prepared = prepare_upload(&rel.tgt, &rel.info)?;
    prepare_category_fields(&rel.tgt, &mut prepared, &rel.info, &rules)?;
    let ctx = DescrOrderCtx {
        source_label: rel.src.site_name.clone(),
        source_url: req.source_url.clone(),
        target_label: rel.tgt.site_name.clone(),
        info: rel.info.clone(),
    };
    apply_site_rules(&mut prepared, &rules, Some(&ctx));
    // 必填缺口 / 图片全剪这类本地硬阻断要计入预检结论，不能只在字段列表下面提示。
    for gap in &prepared.required_gaps {
        rel.preflight.errors.push(gap.clone());
    }
    log(&format!(
        "审阅完成：{} 个字段 → {}（未提交）",
        prepared.fields.len(),
        prepared.action
    ));
    // 站点官方预审（LuckAudit）：把将要提交的字段交给站方打分，结论进审阅报告
    let (audit, audit_note) = if pre_audit_supported(&prepared.form_html) {
        if let Some(t) = throttle {
            if !t.acquire(&req.target_site, BudgetKind::Search) {
                bail!("审阅预算用完（官方预审），本轮跳过该草稿");
            }
        }
        log("调用站点官方预审（LuckAudit）…");
        match run_pre_audit(&rel.tgt, &rel.info, &prepared) {
            Ok(v) => {
                log(&format!("官方预审：{}", v.summary()));
                for d in &v.details {
                    log(&format!("官方预审·{}", d.line()));
                }
                (Some(v), None)
            }
            Err(e) => {
                let note = format!("{e:#}");
                log(&format!("官方预审接口异常（{note}），审阅里只做提示"));
                (None, Some(note))
            }
        }
    } else {
        (None, None)
    };
    Ok(ReviewReport {
        info: rel.info,
        preflight: rel.preflight,
        prepared,
        audit,
        audit_note,
        carry: Some(ReviewCarry {
            src: rel.src,
            tgt: rel.tgt,
            page: rel.page,
        }),
    })
}

// ---------------- 发后自检 ----------------

/// 发后自检：重读新种详情页，核对标题 / MediaInfo / 条目链接真的落上了。
/// 只出警告不推翻上传结论（抓不到页面也算一条警告）。
/// 标题归一化：大小写 + 非字母数字统一成点（站点常把 `.`/空格互换写法）
/// 目标站 IMDb 区判重：很多站按条目判重（同片不同版本也算 existed）。
/// 返回 (详情页链接, 页面标题)；无 IMDb/无命中返回 None。
pub(super) fn target_has_imdb(
    target: &NexusPhp,
    imdb: Option<&str>,
    expected_name: &str,
    log: &mut dyn FnMut(&str),
) -> Result<Option<(String, String)>> {
    let Some(tt) = imdb else {
        return Ok(None);
    };
    if !tt.starts_with("tt") {
        return Ok(None);
    }
    let url = format!(
        "{}/torrents.php?search={}&search_area=4&notnewword=1",
        target.base.as_str().trim_end_matches('/'),
        tt
    );
    let html = match target.fetch_search_html(&url) {
        Ok(h) => h,
        Err(e) => {
            log(&format!("IMDb 区判重失败：{e:#}"));
            return Err(e);
        }
    };
    // 只认搜索结果锚点：<a title="..." href="details.php?id=N...">。
    // 侧栏/用户信息里也有 details.php?id=，不能当判重结果（曾误抓 id=10230）；
    // 侧栏推荐位还可能带 title（13city 的 “Old Fox”），所以还要复核标题。
    Ok(first_imdb_hit(&html, &target.base, expected_name))
}

/// 从 IMDb 搜索页里取第一条与发布名对得上的详情链接。
pub(super) fn first_imdb_hit(
    html: &str,
    base: &Url,
    expected_name: &str,
) -> Option<(String, String)> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(p) = lower[from..].find("<a") {
        let start = from + p;
        let end = html[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(html.len());
        let tag = &html[start..end];
        if let Some(href) = attr(tag, "href") {
            let path = href.split(['?', '#']).next().unwrap_or("");
            let last_seg = path.rsplit('/').next().unwrap_or("");
            if last_seg == "details.php" {
                let Some(pos) = href.find("details.php?id=") else {
                    from = end;
                    continue;
                };
                let after = &href[pos + "details.php?id=".len()..];
                let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    let detail = format!(
                        "{}/details.php?id={digits}",
                        base.as_str().trim_end_matches('/')
                    );
                    let title = attr(tag, "title").unwrap_or_default().to_string();
                    // 侧栏/推荐位也会出现带 title 的 details.php 锚点（例如 13city
                    // 推荐“Old Fox”），只按路径过滤会把它们误当判重命中；这里必须
                    // 再用发布名复核，名称对不上就当没搜到。
                    if !title.is_empty() && search_names_match(&title, expected_name, false) {
                        return Some((detail, title));
                    }
                }
            }
        }
        from = end;
    }
    None
}

// ---------------- 发后定位（只认上传回包自带链接，不做站内检索） ----------------

/// 上传回包里解析出的链接：详情页与下载直链各取首条。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostedLinks {
    pub detail: Option<String>,
    pub download: Option<String>,
}

pub(super) fn collect_id_links(
    haystack: &str,
    base: &Url,
    marker: &str,
    out: &mut Vec<(String, String)>,
) {
    let mut rest = haystack;
    while let Some(p) = rest.find(marker) {
        // `userdetails.php?id=` 里也包含 `details.php?id=`：marker 前面还是字母
        // 或下划线时说明它是更长的文件名，不能当种子详情链接。
        if rest[..p]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            rest = &rest[p + marker.len()..];
            continue;
        }
        let after = &rest[p + marker.len()..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let href = format!("{marker}{digits}");
            // details 链接要躲过 userdetails.php?id= 这类用户主页锚（前缀守卫）
            if marker == "details.php?id=" && !is_detail_href(&href) {
                rest = after;
                continue;
            }
            let u = resolve_url(&href, base);
            if !out.iter().any(|(id, url)| id == &digits && url == &u) {
                out.push((digits, u));
            }
        }
        rest = after;
    }
}

/// 从上传回包正文与最终跳转 URL 提取 `details.php?id=` / `download.php?id=` 链接。
/// 纯解析：不发检索请求、不读「我发布的列表」（QB-TRUTH-MODE §6）。
///
/// 成功页常把侧边栏/相关推荐里的旧种一起带上，不能盲取第一条。取链接的顺序：
/// 1. 最终跳转 URL 是详情页 → 以它为准；下载链接只在 id 对得上时才用；
/// 2. 正文里详情和下载链接 id 相同 → 用这一对；
/// 3. 只有唯一详情或唯一下载 → 用唯一的一个；
/// 4. 多个链接但 id 对不上 → 两个都留空，交给 `posted_unresolved` 人工确认，
///    宁可不挂也不能把旧种挂进 qB。
pub fn extract_posted_links(body: &str, final_url: &str, base: &Url) -> PostedLinks {
    let mut body_details: Vec<(String, String)> = Vec::new();
    let mut body_downloads: Vec<(String, String)> = Vec::new();
    collect_id_links(body, base, "details.php?id=", &mut body_details);
    collect_id_links(body, base, "download.php?id=", &mut body_downloads);

    let mut final_details: Vec<(String, String)> = Vec::new();
    let mut final_downloads: Vec<(String, String)> = Vec::new();
    collect_id_links(final_url, base, "details.php?id=", &mut final_details);
    collect_id_links(final_url, base, "download.php?id=", &mut final_downloads);

    if let Some((fid, furl)) = final_details.first() {
        if let Some((_, durl)) = final_downloads
            .iter()
            .chain(body_downloads.iter())
            .find(|(id, _)| id == fid)
        {
            return PostedLinks {
                detail: Some(furl.clone()),
                download: Some(durl.clone()),
            };
        }
        return PostedLinks {
            detail: Some(furl.clone()),
            download: None,
        };
    }
    if let [(did, durl)] = final_downloads.as_slice() {
        if let Some((_, detail)) = body_details.iter().find(|(id, _)| id == did) {
            return PostedLinks {
                detail: Some(detail.clone()),
                download: Some(durl.clone()),
            };
        }
        return PostedLinks {
            detail: None,
            download: Some(durl.clone()),
        };
    }

    // 正文里 id 对得上的详情/下载对。唯一一对才可信。
    let pairs: Vec<_> = body_downloads
        .iter()
        .filter_map(|d| {
            body_details
                .iter()
                .find(|(id, _)| id == &d.0)
                .map(|detail| (detail, d))
        })
        .collect();
    if let [(detail, download)] = pairs.as_slice() {
        return PostedLinks {
            detail: Some(detail.1.clone()),
            download: Some(download.1.clone()),
        };
    }
    if !pairs.is_empty() {
        return PostedLinks::default();
    }

    // 只有一类链接时，唯一的那条还能用；多条就分不清新旧。
    if body_downloads.is_empty() && body_details.len() == 1 {
        return PostedLinks {
            detail: Some(body_details[0].1.clone()),
            download: None,
        };
    }
    if body_details.is_empty() && body_downloads.len() == 1 {
        return PostedLinks {
            detail: None,
            download: Some(body_downloads[0].1.clone()),
        };
    }
    PostedLinks::default()
}

/// 发后定位的站内检索兜底：按发布名 + 本地总大小找刚发布的新种。
///
/// 只在 takeupload.php 回包/跳转没有给出详情或下载链接时使用。命中后先核对详情页标题，
/// 再核对详情页大小（结果行大小解析不出来时也要过这一关），最后只下载站点生成的
/// `.torrent`；不会拿本地上传构造的种子顶替站种，避免把审核没过的废种挂进 qB。
/// `held` 是出参：候选页面找到了、但站点因为审核没放行不给下载时，记下（详情页，审核结论），
/// 调用方据此落“等站点审核”，而不是“定位不到新种”。
pub(super) fn locate_posted_by_search(
    tgt: &NexusPhp,
    info: &ReleaseInfo,
    expected_size: Option<i64>,
    relaxed_retry: bool,
    throttle: &SiteThrottle,
    log: &mut dyn FnMut(&str),
    held: &mut Option<(String, ReviewMark, String)>,
) -> Option<(String, Vec<u8>)> {
    let expected_title = form::title_for_site(info, &tgt.site_id);
    for relaxed in [false, true] {
        if relaxed && !relaxed_retry {
            break;
        }
        let mut gate = || throttle.acquire(&tgt.site_id, BudgetKind::Search);
        let outcome = match tgt.search_sized(
            &expected_title,
            "",
            relaxed,
            info.imdb.as_deref().unwrap_or(""),
            expected_size,
            &mut gate,
        ) {
            Ok(SearchOutcome::Hit(h)) => h,
            Ok(SearchOutcome::Miss) => continue,
            Ok(SearchOutcome::BudgetExhausted) => {
                log("发后定位·检索预算用完，转 posted_unresolved");
                return None;
            }
            Err(e) => {
                log(&format!("发后定位·站内检索失败（{e:#}）"));
                return None;
            }
        };

        if !throttle.acquire(&tgt.site_id, BudgetKind::Search) {
            log("发后定位·详情页预算用完，转 posted_unresolved");
            return None;
        }
        let page = match tgt.fetch_torrent_page(&outcome.detail_url) {
            Ok(p) => p,
            Err(e) => {
                log(&format!("发后定位·候选详情页读不下来（{e:#}）"));
                continue;
            }
        };
        let page_title = strip_site_title_badges(&page.name);
        if !search_names_match(page_title, &expected_title, true)
            && norm_post_title(page_title) != norm_post_title(&expected_title)
        {
            log(&format!(
                "发后定位·候选标题不符：页面「{}」，期望「{}」",
                page.name, expected_title
            ));
            continue;
        }
        if let Some(expected) = expected_size {
            match page.size_bytes {
                Some(actual)
                    if !sizes_close_with_slack(actual, expected, page.size_slack_bytes) =>
                {
                    log(&format!(
                        "发后定位·候选大小不符：页面 {} 字节，期望 {} 字节",
                        actual, expected
                    ));
                    continue;
                }
                None => log("发后定位·详情页大小解析不到，继续用 pieces 核对"),
                _ => {}
            }
        }

        if !throttle.acquire(&tgt.site_id, BudgetKind::Search) {
            log("发后定位·下载预算用完，转 posted_unresolved");
            return None;
        }
        match tgt.download_torrent(&page, &outcome.detail_url) {
            Ok(data) => {
                log(&format!(
                    "发后定位·站内检索命中：{}（{}）",
                    outcome.detail_url, page.name
                ));
                return Some((outcome.detail_url, data));
            }
            Err(e) => match site_review_hold(&e) {
                // 新种找到了，只是站点要先审才给下载：这就是刚发的那条，记下交给调用方落格
                Some((mark, why)) => {
                    *held = Some((outcome.detail_url.clone(), mark, why));
                    return None;
                }
                None => log(&format!("发后定位·候选站点种下载失败（{e:#}）")),
            },
        }
    }
    None
}

/// 下载失败是不是因为站点审核没放行（`SiteNotice`，且提示里认出了审核结论）。
/// “通过”不算：站点标着通过却不给下载，那是别的问题，走普通的下载失败路径。
pub(super) fn site_review_hold(err: &anyhow::Error) -> Option<(ReviewMark, String)> {
    err.downcast_ref::<SiteNotice>()?
        .review
        .clone()
        .filter(|(m, _)| *m != ReviewMark::Approved)
}

/// 站点收下了种子，但审核没放行（要先审，或已经判了不通过）：拿不到站点种子，挂不了 qB。
/// 这不是发种失败：不记 failed、不进失败冷却、不重发（重发只会得到重复种）；
/// 落专门的格子，等回访（`bsend followup` / 自动发种每轮开头）收尾。
pub(super) fn review_hold_outcome(
    index: &mut ContentIndex,
    key: &str,
    site: &str,
    url: Option<String>,
    mark: ReviewMark,
    why: &str,
    log: &mut dyn FnMut(&str),
) -> Result<ReseedOutcome> {
    let (status, note) = match mark {
        ReviewMark::Pending => (
            CoverageStatus::PendingReview,
            format!("站上已发布，等站点审核（站点原话：{why}）；审完前拿不到站点种子，没挂回 qB"),
        ),
        ReviewMark::Rejected => (
            CoverageStatus::ReviewRejected,
            format!("站点审核没过（{why}）；没挂回 qB。改简介或种子后在站上处理，程序不重发"),
        ),
        // site_review_hold 已经把“通过”滤掉了；真走到这里，说明不是审核在等，按原文记下来让人看
        ReviewMark::Approved => (
            CoverageStatus::ReviewPending,
            format!("posted_unresolved：站点页面标着“通过”却不给下载（{why}），待人工确认；勿重发"),
        ),
    };
    log(&format!("站点审核·{}：{}", status.label(), note));
    write_cell(index, key, site, status, url.clone(), &note);
    index.save()?;
    Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
        mounted: false,
        status,
        url,
        note,
    }))
}

/// 站点让下载、种子也挂回 qB 在做种了，但页面上还标着审核结论（tey：`title="未审"`；
/// 麒麟：“审核不通过，原因：…”）。做种不受影响，可这条种子留不留得下来要看审核，
/// 所以格子按审核结论落，等回访收尾。
pub(super) fn review_seeding_outcome(
    index: &mut ContentIndex,
    key: &str,
    site: &str,
    detail_url: &str,
    mark: ReviewMark,
    why: &str,
    log: &mut dyn FnMut(&str),
) -> Result<ReseedOutcome> {
    let (status, note) = match mark {
        ReviewMark::Pending => (
            CoverageStatus::PendingReview,
            format!("站上已发布，种子已挂回 qB 做种；站点页面还标着“{why}”，等审核结果"),
        ),
        ReviewMark::Rejected => (
            CoverageStatus::ReviewRejected,
            format!("站点审核没过：{why}；种子已挂回 qB，仍在做种"),
        ),
        // 调用前已经把“通过”滤掉了；标着通过就是正常发布完成，格子不动
        ReviewMark::Approved => return Ok(ReseedOutcome::Posted(detail_url.to_string())),
    };
    log(&format!("站点审核·{}：{}", status.label(), note));
    write_cell(
        index,
        key,
        site,
        status,
        Some(detail_url.to_string()),
        &note,
    );
    index.save()?;
    Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
        mounted: true,
        status,
        url: Some(detail_url.to_string()),
        note,
    }))
}

/// existed 回包详情页标题核对：只采信回包给的 hint 页面，标题一致才返回。
/// 这里不做站内检索校正；新发后的无链接兜底检索由 `locate_posted_by_search` 负责。
/// 不用 IMDb 区兜底：IMDb 命中只说明同一部片子，不能说明同一条种子。
pub(super) fn locate_existing_target(
    tgt: &NexusPhp,
    info: &ReleaseInfo,
    hint: &str,
    log: &mut dyn FnMut(&str),
) -> Option<String> {
    if hint.is_empty() {
        return None;
    }
    match tgt.fetch_torrent_page(hint) {
        Ok(p) => {
            let page_title = strip_site_title_badges(&p.name);
            let expected_title = form::title_for_site(info, &tgt.site_id);
            if norm_post_title(page_title) == norm_post_title(&expected_title) {
                Some(hint.to_string())
            } else {
                log(&format!(
                    "existed 回包详情页标题不符（页面「{}」，用于比对「{}」），弃用该线索",
                    p.name, expected_title
                ));
                None
            }
        }
        Err(e) => {
            log(&format!("existed 回包详情页打不开（{e:#}），弃用该线索"));
            None
        }
    }
}

/// 发后自检：回读新种页核对标题/MediaInfo/链接。同一次请求顺带带回页面上明写的审核结论
/// （未审 / 审核不通过），不额外打站点。抓不到页面时只返回一条警告，审核结论为 `None`。
pub fn verify_posted(
    target: &NexusPhp,
    detail_url: &str,
    info: &ReleaseInfo,
) -> (Vec<String>, Option<(ReviewMark, String)>) {
    match target.fetch_html(detail_url, target.base.as_str()) {
        Ok(html) => (
            verify_posted_html(&html, info, &target.site_id),
            review_mark(&html, &form::title_for_site(info, &target.site_id)),
        ),
        Err(e) => (
            vec![format!("抓新种详情页失败（{e:#}），请手动核对发布结果")],
            None,
        ),
    }
}

/// 发后自检的纯文本比对（单测友好）
pub(super) fn verify_posted_html(html: &str, info: &ReleaseInfo, site_id: &str) -> Vec<String> {
    let mut warns = Vec::new();
    let posted = parse_release_page(html, "");
    // 标题
    let want = form::title_for_site(info, site_id);
    let want = want.trim();
    let got = posted.name.trim();
    if got.is_empty() {
        warns.push("新种页没解析出标题，无法核对".into());
    } else if !want.is_empty() && got != want {
        warns.push(format!("标题与预期不一致：预期「{want}」页面是「{got}」"));
    }
    // MediaInfo：独立块没有就退化成「页面里出现了原文行」
    if !info.mediainfo.trim().is_empty() {
        let landed = !posted.mediainfo.trim().is_empty()
            || info
                .mediainfo
                .lines()
                .map(str::trim)
                .filter(|l| l.len() >= 8)
                .take(5)
                .any(|l| html.contains(l));
        if !landed {
            warns.push("MediaInfo 没落到新种页：确认技术信息/简介里带上了".into());
        }
    }
    // 条目链接（按 id 本体找，页面展示形式可能带完整 URL）
    if let Some(tt) = &info.imdb {
        if !html.contains(tt) {
            warns.push(format!("IMDb id {tt} 没在新种页出现"));
        }
    }
    if let Some(db) = &info.douban {
        if !html.contains(db) {
            warns.push(format!("豆瓣 id {db} 没在新种页出现"));
        }
    }
    if let Some(tm) = &info.tmdb {
        let tail = tm.rsplit('/').next().unwrap_or("");
        if !tail.is_empty() && !html.contains(tail) {
            warns.push("TMDB 链接没在新种页出现".into());
        }
    }
    warns
}
