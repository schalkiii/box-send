//! 上传表单提交与失败回包解析。

use super::*;

pub fn prepare_upload(target: &NexusPhp, info: &ReleaseInfo) -> Result<PreparedUpload> {
    let form_url = resolve_url("upload.php", &target.base);
    let form_html = target
        .fetch_html(&form_url, target.base.as_str())
        .context("抓目标站上传表单失败")?;
    let kind = classify_upload_page(&form_html);
    if let Some(msg) = gate_error_message(kind, &target.site_id, &form_url, &form_html) {
        bail!("{msg}");
    }
    if let Some(notice) = crate::sites::nexusphp::block_notice(&form_html) {
        bail!("目标站限流（{notice}）：{form_url}");
    }

    let tags_wanted = wanted_tags(info);
    let (mut fields, picked_tags, select_notes) =
        build_upload_fields_for_site(info, &form_html, &tags_wanted, &target.site_id);
    fill_image_fields(
        &mut fields,
        &form_html,
        &info.descr,
        target.poster_field.as_deref(),
        target.screenshot_field.as_deref(),
    );
    // 想勾的标签一个都没匹配上才警告（复选框 value 多是数字编码，
    // 拿 value 里含不含关键词来判会把勾上了的误报成漏勾）
    let tag_warn = if !tags_wanted.is_empty() && picked_tags.is_empty() {
        Some(format!(
            "[{}] 表单里没找到「{}」类标签复选框，请发布后手动补",
            target.site_id,
            tags_wanted.join("/")
        ))
    } else {
        None
    };

    let action = form_action(&form_html, &target.base).unwrap_or_else(|| form_url.clone());
    if let Some(bad) = non_upload_endpoint(&action, &target.base) {
        bail!("上传表单 action 不是上传处理页：{action}（命中 {bad}），已中止提交");
    }
    Ok(PreparedUpload {
        action,
        form_url,
        fields,
        picked_tags,
        tag_warn,
        select_notes,
        required_gaps: vec![],
        form_html,
        poster_field: target.poster_field.clone(),
        screenshot_field: target.screenshot_field.clone(),
    })
}

/// 提交到目标站 upload.php。multipart 一次提交 + 「已丢失 POST」重投一次
/// （Swift 版实测吐鲁番连续提交会偶发丢 POST，同字节重投即可）。
pub fn upload_release(
    target: &NexusPhp,
    info: &ReleaseInfo,
    torrent_data: &[u8],
    filename: &str,
    dump_dir: Option<&Path>,
) -> Result<UploadResult> {
    let prep = prepare_upload(target, info)?;
    submit_upload(target, &prep, torrent_data, filename, dump_dir)
}

/// 上传回包调试落盘开关：仅 `BSEND_DUMP_UPLOAD=1/true/on` 时开启，默认关闭。
/// 抽成纯函数便于单测，开关判定不直接读环境变量本身。
pub(super) fn dump_upload_enabled(env: Option<&str>) -> bool {
    matches!(
        env.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("on")
    )
}

pub(super) fn upload_dump_enabled() -> bool {
    dump_upload_enabled(std::env::var("BSEND_DUMP_UPLOAD").ok().as_deref())
}

/// 把上传回包（状态 / 最终 URL / 非凭据响应头 / 提交字段名 / 正文）写到数据目录
/// `upload-debug/<site>-<unix>-a<attempt>.html`。开关关闭或目录不可用时静默跳过，
/// 不影响主流程；快照已剔除 Cookie/Set-Cookie，正文不含 passkey。
pub(super) fn dump_upload_response(
    dump_dir: Option<&Path>,
    site_id: &str,
    resp: &crate::sites::nexusphp::UploadResponse,
    action: &str,
    fields: &[Field],
    attempt: u8,
) {
    if !upload_dump_enabled() {
        return;
    }
    let Some(dir) = dump_dir else { return };
    let dir = dir.join("upload-debug");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stamp = now_unix();
    let file = dir.join(format!("{site_id}-{stamp}-a{attempt}.html"));
    let field_names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
    let mut preamble = String::new();
    preamble.push_str("<!-- BoxSend upload debug dump\n");
    preamble.push_str(&format!("site: {site_id}\n"));
    preamble.push_str(&format!("time_unix: {stamp}\n"));
    preamble.push_str(&format!("attempt: {attempt}\n"));
    preamble.push_str(&format!("action: {action}\n"));
    preamble.push_str(&format!("status: {}\n", resp.status));
    preamble.push_str(&format!("final_url: {}\n", resp.final_url));
    preamble.push_str("headers:\n");
    for (k, v) in &resp.headers {
        preamble.push_str(&format!("  {k}: {v}\n"));
    }
    preamble.push_str(&format!(
        "submitted_field_names: {}\n",
        field_names.join(", ")
    ));
    preamble.push_str("-->\n");
    let body = format!("{preamble}{}", resp.body);
    let _ = std::fs::write(&file, body);
}

/// 上传被拒且没解析出具体原因时，把回包正文落到系统临时目录，供事后查拒因。
/// 文件名 `bsend-upload-<站点id>-<毫秒时间戳>.html`，UTF-8 原样写。
/// 写盘失败静默忽略（不把原本的错误吃掉）；成功才返回绝对路径。
pub(super) fn dump_upload_body_to_temp(site_id: &str, body: &str) -> Option<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let mut file = std::env::temp_dir();
    file.push(format!("bsend-upload-{site_id}-{stamp}.html"));
    std::fs::write(&file, body.as_bytes()).ok()?;
    Some(file)
}

/// 剥掉 HTML 标签、折叠空白后取前 max 个字符的可见文字；剥不出返回空串。
pub(super) fn visible_text_snippet(html: &str, max: usize) -> String {
    let collapsed: String = strip_tags(html)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.chars().take(max).collect()
}

/// 这些是站点没给出明确拒因的兜底结论：回包正文另落临时目录备查。
pub(super) fn is_generic_upload_error(msg: &str) -> bool {
    msg.starts_with("站点拒绝（回包未展开具体原因）：")
        || (msg.starts_with("HTTP ") && msg.contains("未识别的返回"))
}

/// HTTP 200 但回包认不出来：这是「未确认发布」，不是明确的站点拒绝。
/// 403/404 等状态码有明确含义，空回包按后端超时处理，都不走这条兜底。
pub(super) fn is_unconfirmed_http_200(msg: &str) -> bool {
    msg.starts_with("HTTP 200 未识别的返回")
}

/// 兜底拒因：把回包正文落临时目录，并在错误信息里附路径和一小段可见文字。
/// 已能解析出具体原因的错误不走这里，文案与行为不变；写盘失败静默忽略。
pub(super) fn augment_upload_error_with_dump(site_id: &str, body: &str, msg: &str) -> String {
    if !is_generic_upload_error(msg) {
        return msg.to_string();
    }
    let Some(path) = dump_upload_body_to_temp(site_id, body) else {
        return msg.to_string();
    };
    let snippet = visible_text_snippet(body, 200);
    if snippet.is_empty() {
        format!("{msg}；回包已存 {}", path.display())
    } else {
        format!("{msg}；回包已存 {}；可见文字：{snippet}", path.display())
    }
}

/// 用已构造好的字段提交（`prepare_upload` 的产出）。官方预审、审阅与
/// 正式转种共用同一份字段，永不漂移。
pub fn submit_upload(
    target: &NexusPhp,
    prep: &PreparedUpload,
    torrent_data: &[u8],
    filename: &str,
    dump_dir: Option<&Path>,
) -> Result<UploadResult> {
    // 成功消息末尾追加标签警告（若有）
    let note_suffix = prep
        .tag_warn
        .as_deref()
        .map(|w| format!("；{w}"))
        .unwrap_or_default();
    let mut last_err = String::new();
    for attempt in 0..2 {
        let resp = match target.post_upload(
            &prep.action,
            &prep.fields,
            "file",
            filename,
            torrent_data,
            &prep.form_url,
        ) {
            Ok(resp) => resp,
            Err(e) => {
                // 网络层超时/断连没有回包，不能像站点拒绝那样直接判死；
                // 同一字节再投一次，第二次仍不通才报失败。
                if attempt == 0 {
                    last_err = format!("{e:#}");
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
                return Err(e.context(format!("提交上传失败（第 {} 次）", attempt + 1)));
            }
        };
        dump_upload_response(
            dump_dir,
            &target.site_id,
            &resp,
            &prep.action,
            &prep.fields,
            attempt as u8,
        );
        let body = resp.body.clone();
        let final_url = resp.final_url.clone();
        // 「已存在」先判：站点常跳回已有种子详情页，先判成功会误报转种成功。
        // 只认结构性证据（existed=1 跳转 / errorbox），整页自由文本不能当重复，
        // 否则普通校验失败页里的模板措辞会被误判成「站点已有」。
        if let Some(marker) = exist_marker(&final_url, &body) {
            let dup_url = sole_detail_url(&body, &final_url, &prep.action);
            return Ok(UploadResult {
                detail_url: dup_url,
                message: format!(
                    "站点提示种子已存在（证据：{marker}；视为已覆盖，不算本次转种）{note_suffix}"
                ),
                duplicate: true,
                uncertain: false,
                body,
                final_url,
            });
        }
        // 成功：跳到详情页 / 发布成功提示
        if let Some(u) = redirect_detail_url(&final_url, &prep.action) {
            return Ok(UploadResult {
                detail_url: Some(u),
                message: format!("发布成功（跳转详情页）{note_suffix}"),
                duplicate: false,
                uncertain: false,
                body,
                final_url,
            });
        }
        if body.contains("发布成功")
            || body.contains("new torrent")
            || body.contains("Torrent added")
        {
            let u = sole_detail_url(&body, &final_url, &prep.action);
            return Ok(UploadResult {
                detail_url: u.clone(),
                message: if u.is_some() {
                    format!("发布成功{note_suffix}")
                } else {
                    format!("发布成功（响应页没找到详情链接，需手动确认新种）{note_suffix}")
                },
                duplicate: false,
                uncertain: false,
                body,
                final_url,
            });
        }
        // 部分站点成功后不发 Location 跳转，回包正文挂新种详情页链接（kufei 实测：
        // 提交后 final_url 仍是上传页，正文带 details.php?id=）。回包不是上传表单
        // 重填页、且正文只有唯一 details.php?id=N 时视为成功。这里只放宽「成功
        // 识别方式」：existed 判定与判重口径不变。
        if let Some(u) = body_detail_success_url(&body, &final_url, &prep.action) {
            return Ok(UploadResult {
                detail_url: Some(u),
                message: format!("发布成功（回包正文详情链接）{note_suffix}"),
                duplicate: false,
                uncertain: false,
                body,
                final_url,
            });
        }
        last_err = extract_upload_error(&body, resp.status);
        last_err = augment_upload_error_with_dump(&target.site_id, &body, &last_err);
        // HTTP 200 但页面认不出来：先别判失败。站点可能已经收下种，只是成功页/拒绝页
        // 没有可识别的结构；交给发后定位按名称+总大小检索，找到就挂回，找不到落
        // `posted_unresolved` 待人工确认。403/404 和空回包仍有明确含义，不走这条。
        if is_unconfirmed_http_200(&last_err) {
            return Ok(UploadResult {
                detail_url: None,
                message: format!(
                    "上传回包未识别（{last_err}）；未确认发布，继续按名称+总大小站内核对"
                ),
                duplicate: false,
                uncertain: true,
                body,
                final_url,
            });
        }
        if looks_like_lost_post(&last_err) && attempt == 0 {
            thread::sleep(Duration::from_secs(2));
            continue; // 同字节重投
        }
        break;
    }
    Err(anyhow!("上传未成功：{last_err}"))
}

/// 上传未成功时提取错误（errorbox → 常见错误特征 → 兜底）
/// 站点把失败原因放在 `<h1>上传失败！</h1>` 后面的 `<p>` 里（UBits 实测）。
/// 这种页面的 `<title>` 也含“上传失败”，按普通关键词截取只会拿到标题，
/// 必须从 h1 后面取正文，才能把“黑名单图床”这类真实原因带进审计。
pub(super) fn upload_failure_detail(body: &str) -> Option<String> {
    let h1 = body.find("<h1")?;
    let open_end = h1 + body[h1..].find('>')?;
    let close = open_end + body[open_end..].find("</h1>")?;
    let heading = strip_tags(&body[open_end + 1..close]);
    if !heading.contains("上传失败") && !heading.contains("发布失败") {
        return None;
    }
    let tail = &body[close + 5..];
    let end = tail
        .find("</td>")
        .or_else(|| tail.find("</div>"))
        .unwrap_or_else(|| tail.len().min(800));
    let text = strip_tags(&tail[..end]);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    if line.is_empty()
        || line == "上传失败！"
        || line == "发布失败！"
        || line.contains("Powered by NexusPHP")
    {
        return None;
    }
    Some(format!("站点拒绝：{line}"))
}

pub(super) fn extract_upload_error(body: &str, status: u16) -> String {
    // 种子过大：NexusPHP 系不回错误框，而是回一个 `torrent_too_large` 表单自动跳
    // offers.php（候选区）。这是结构性信号，优先于空壳判断：表单本身没有可见文本，
    // 按普通“未识别返回”记会把站点明确的结论埋掉。
    if body.contains("torrent_too_large") || body.contains("add_offer=1") {
        return "站点判定种子过大（torrent_too_large），要求改走候选（offers.php）；不自动重试"
            .into();
    }
    // 空回包不是“未知页面”：tjupt 对超大种子处理到后端超时后会回 HTTP 200、
    // 正文 0 字节；有的站只回一个没有可见文本的 HTML 空壳。两种都按“后端没给结论”
    // 记，不能混成站点拒绝，也不能让人误以为站点静默通过。
    let visible = strip_tags(body);
    if body.trim().is_empty() || visible.trim().is_empty() {
        return format!(
            "HTTP {status} 空回包（站点后端没有返回内容，通常是处理超时或崩溃；本次未确认发布）"
        );
    }
    if let Some(t) = div_inner_by_key(body, "errorbox") {
        let t = strip_tags(&t).trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    if let Some(detail) = upload_failure_detail(body) {
        return detail;
    }
    // 缺必填项类拒绝：站点明写某项没填（tjupt 编辑页实测回“有项目没有填写”）。
    // 和后端超时分开报：这是站点明确的校验拒绝，回包里有正文；超时是没有结论。
    const MISSING_FIELD_MARKERS: &[&str] = &[
        "有项目没有填写",
        "没有填写",
        "请填写必填",
        "必填项目",
        "必填字段",
    ];
    if let Some(marker) = MISSING_FIELD_MARKERS.iter().find(|m| body.contains(**m)) {
        let snippet = visible
            .lines()
            .find(|l| l.contains(*marker))
            .unwrap_or(marker)
            .trim()
            .to_string();
        return format!("站点提示缺少必填项：{snippet}");
    }
    for marker in ["已经上传", "已存在", "被人上传", "上传失败", "发布失败"] {
        if let Some(p) = body.find(marker) {
            let seg = strip_tags(&body[p..floor_char_boundary(body, p + 120)]);
            let line = seg.lines().next().unwrap_or("").trim().to_string();
            if !line.is_empty() {
                return format!("站点拒绝（回包未展开具体原因）：{line}");
            }
        }
    }
    if body.to_lowercase().contains("takelogin.php") || body.contains("signinhandler") {
        return "会话失效（返回登录页），请重新导入该站 cookie".into();
    }
    if body.contains("<form") && body.contains("upload.php") {
        return "服务器未跳转到详情页（必填字段缺失或校验失败）".into();
    }
    format!("HTTP {status} 未识别的返回")
}

/// 上传错误信息是不是“站点嫌种子太大、要求走候选”（终态，不重试）。
pub(super) fn site_rejects_size(msg: &str) -> bool {
    msg.contains("torrent_too_large") || msg.contains("种子过大")
}

/// 站点「种子已存在」判定。返回命中的证据描述（写进审计，便于回溯）。
///
/// 只认两类结构性证据：
/// 1. 跳转落到 `details.php?id=…&existed=1`（NexusPHP 重复上传的标准动作）；
/// 2. `errorbox` 里出现重复措辞。
///
/// 过去是整页 `contains`，结果把导航/表单说明/帮助文案里的「已存在」也算命中：
/// 一旦上传页因必填项校验失败被原样返回，就会被记成「站点已有」，凭空造出一条
/// 并不存在的详情页链接。注意：`existed=1` 也只证明「站点认为重复」，不证明
/// 那个 id 是本次内容的条目——回包 id 在实测里 6/6 不可信，仍需逐条核对。
pub(super) fn exist_marker(final_url: &str, body: &str) -> Option<&'static str> {
    if final_url.to_ascii_lowercase().contains("existed=1") {
        return Some("跳转 URL 带 existed=1");
    }
    // 有些站把「该种子已存在！」放在重填页的提示区，正文仍带 multipart 上传表单；
    // 单看 errorbox 或短页面会漏掉。这里只认带「该种子」的完整句子，避免把
    // 上传页帮助文案里的「重复资源会提示种子已存在」误判成重复。
    const EXACT_NOTICES: &[&str] = &[
        "该种子已存在！",
        "该种子已经存在！",
        "该种子已存在",
        "该种子已经存在",
        // 少数站只写“已存在！”；带感叹号比裸“已存在”更像站点提示，
        // 放在精确句后面兜底，避免长页面/重填页漏判。
        "已存在！",
        "已经存在！",
    ];
    const MARKERS: &[&str] = &[
        "已经上传",
        "已存在",
        "被人上传",
        "already exists",
        "Torrent already",
    ];
    // 错误框里的明确重复措辞优先：有的站会把错误框和上传表单一起返回（重填页），
    // 不能因为看到 multipart 就忽略站点说过的「已存在」。
    if let Some(boxed) = div_inner_by_key(body, "errorbox") {
        if let Some(m) = MARKERS.iter().find(|m| boxed.contains(**m)) {
            return Some(*m);
        }
    }
    // 返回体还是上传表单（multipart）：这是校验失败重填，不是重复提示；
    // 但若重填页的普通提示区写着带「该种子」的完整句子，仍要认成重复。
    if body.contains("multipart/form-data") {
        if let Some(m) = EXACT_NOTICES.iter().find(|m| body.contains(**m)) {
            return Some(*m);
        }
        return None;
    }
    // 提示页通常很短；长页面里的零散措辞多半是模板/简介文本
    if body.len() <= 8192 {
        if let Some(m) = MARKERS.iter().find(|m| body.contains(**m)) {
            return Some(*m);
        }
    }
    // 有的站长页面（带页头页脚、超过 8KB）只放一句「该种子已存在！」，
    // 没有 errorbox 也没有 multipart；这时仍按精确完整句认重复。
    if let Some(m) = EXACT_NOTICES.iter().find(|m| body.contains(**m)) {
        return Some(*m);
    }
    None
}

pub(super) fn looks_like_lost_post(msg: &str) -> bool {
    msg.contains("请填写必填项目") || msg.contains("必填字段缺失")
}

/// 最终 URL / 响应体里的详情页链接（details.php?id=N）
pub(super) fn redirect_detail_url(final_url: &str, action: &str) -> Option<String> {
    if final_url == action {
        return None;
    }
    let lower = final_url.to_lowercase();
    if lower.contains("details.php?id=") || lower.contains("torrents.php?id=") {
        // 不能是登录页/上传页
        if !lower.contains("upload") && !lower.contains("login") {
            return Some(final_url.to_string());
        }
    }
    None
}

/// 成功但没跳转：从回包正文取唯一详情页链接。上传表单重填页（校验失败回显，
/// multipart 或仍含文件上传框）不参与——侧边栏挂的旧种链接不能当新种证据。
pub(super) fn body_detail_success_url(body: &str, final_url: &str, action: &str) -> Option<String> {
    if body.contains("multipart/form-data") || upload_form_block(body).is_some() {
        return None;
    }
    sole_detail_url(body, final_url, action)
}

pub(super) fn sole_detail_url(body: &str, final_url: &str, action: &str) -> Option<String> {
    if let Some(u) = redirect_detail_url(final_url, action) {
        return Some(u);
    }
    // 响应体里唯一的 details.php?id=N；`userdetails.php?id=` 是用户主页，不算。
    let base = Url::parse(action).ok()?;
    let mut found: Vec<(String, String)> = Vec::new();
    collect_id_links(body, &base, "details.php?id=", &mut found);
    if found.len() == 1 {
        Some(found.pop().unwrap().1)
    } else {
        None
    }
}

// ---------------- 审阅模式（不提交） ----------------

/// 审阅时已取到的发布侧载荷：源/目标适配器与源页。
/// 发布器直接复用它们，既不再抓源页/上传页，也保证提交字段与门禁判定是同一份。
pub(crate) struct ReviewCarry {
    pub(super) src: NexusPhp,
    pub(super) tgt: NexusPhp,
    pub(super) page: TorrentPage,
}
