//! 表单静态演练、官方预审请求与响应解析。
//!
//! 从 `reseed.rs` 拆出；不执行真实上传。

use super::*;

#[derive(Debug, Default)]
pub struct FormDrill {
    pub site_id: String,
    pub site_name: String,
    /// 表单没拿到/构造失败的原因（登录墙、限流、无上传权限…）
    pub error: Option<String>,
    /// 失败类别：`no_permission` / `two_fa` / `challenge_gate` / `spa_api` /
    /// `login` / `no-form` / `network`
    pub error_kind: Option<String>,
    pub field_count: usize,
    pub select_count: usize,
    pub title_set: bool,
    /// 标题输入框在表单里不存在（tjupt 实测：标题由种子文件名自动生成）
    pub title_auto: bool,
    pub subtitle_set: bool,
    pub descr_bytes: usize,
    /// MediaInfo 落到哪：独立字段名，或「内嵌简介」
    pub mediainfo_field: Option<String>,
    /// 条目链接落到的字段名
    pub link_fields: Vec<String>,
    pub picked_tags: Vec<String>,
    pub tag_warn: Option<String>,
    /// 自动选档结果：(字段, 选项文案)
    pub select_picks: Vec<(String, String)>,
    /// 仍为 0/空的下拉（缺口与否由 core_select_gaps 归类）
    pub open_selects: Vec<String>,
    /// 硬缺口下拉（前缀级折叠；域无关与垂直站分类不算）
    pub core_select_gaps: Vec<String>,
    pub notes: Vec<String>,
    /// 上传页是否带官方预审面板（LuckAudit）
    pub audit_panel: bool,
    /// 表单解析出的提交地址（绝对 URL）；构造失败时为 None
    pub action: Option<String>,
    /// 每个上传下拉的全量选项：(下拉名, [(value, label)])，供 formcheck 落盘核对缺口
    pub select_options: Vec<(String, Vec<(String, String)>)>,
}

impl FormDrill {
    /// 硬缺口：分类 + 核心质量下拉仍为 0/空，或名称/简介/MediaInfo 没落上。
    /// 标题由站点自动生成（无标题输入框）、垂直站分类不适配不算缺口。
    pub fn core_gaps(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(e) = &self.error {
            out.push(e.clone());
            return out;
        }
        if !self.title_set && !self.title_auto {
            out.push("标题未落上".into());
        }
        if self.descr_bytes == 0 {
            out.push("简介为空".into());
        }
        if self.mediainfo_field.is_none() {
            out.push("MediaInfo 无处可落".into());
        }
        for s in &self.core_select_gaps {
            out.push(format!("{s} 未选"));
        }
        out
    }

    /// 软缺口：制作组/处理/地区等次要或域无关下拉未选、选档提示
    pub fn soft_gaps(&self) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.open_selects {
            // 已按前缀点名进硬缺口的不重复（多组同前缀只报一次）
            let covered = self
                .core_select_gaps
                .iter()
                .any(|c| s == c || (s.starts_with(c) && s[c.len()..].starts_with('[')));
            if !covered {
                out.push(format!("{s} 未选"));
            }
        }
        out.extend(self.notes.iter().cloned());
        if let Some(w) = &self.tag_warn {
            out.push(w.clone());
        }
        out
    }
}

/// 演练样本：覆盖 中字/国产剧/HDTV/1080p/x264/AAC/制作组/IMDb/豆瓣/MediaInfo 全要素
pub fn drill_release() -> ReleaseInfo {
    ReleaseInfo {
        name: "The.Disguiser.Complete.1080p.HDTV.x264.AAC-JING".into(),
        subtitle: "国产剧--伪装者[无台标完整版 全41集1080P][中字]".into(),
        descr: "演练样本简介（中字）。IMDb：https://www.imdb.com/title/tt5154296/\n豆瓣：https://movie.douban.com/subject/25823432/\n截图：<img src=\"https://img.example/shot01.jpg\"><img src=\"https://img.example/shot02.jpg\"><img src=\"https://img.example/shot03.jpg\">".into(),
        mediainfo: "General\nUnique ID : 1\nFormat : Matroska\nDuration : 47mn 17s\nOverall bit rate : 6000 kb/s\n\nVideo\nFormat : AVC\nFormat/Info : Advanced Video Codec\nWidth : 1920 pixels\nHeight : 1080 pixels\n\nAudio\nLanguage : Chinese\nFormat : AAC\nChannel(s) : 2 channels".into(),
        imdb: Some("tt5154296".into()),
        douban: Some("25823432".into()),
        tmdb: None,
        completed_hint: true,
    }
}

/// 逐站演练：与真实转种同一套字段构造（prepare_upload），但绝不 POST。
/// 站点规则缺省时用空规则，行为与旧版一致。
pub fn drill_upload_form(target: &NexusPhp) -> FormDrill {
    drill_upload_form_with_rules(target, &crate::gates::SiteRules::default())
}

/// 逐站演练（带站点规则）：与 gatecheck 同一套——拉表单 → 分类附加字段 →
/// 应用站点规则（选档回填/必填校验/剪死图/独立图片字段）→ 分析缺口。
/// formcheck 与 gatecheck 走这同一套，两边缺口结论才不会打架。
pub fn drill_upload_form_with_rules(
    target: &NexusPhp,
    rules: &crate::gates::SiteRules,
) -> FormDrill {
    let info = drill_release();
    match prepare_upload(target, &info) {
        Ok(mut prep) => {
            let _ = prepare_category_fields(target, &mut prep, &info, rules);
            apply_site_rules(&mut prep, rules, None);
            analyze_form(&target.site_id, &target.site_name, &prep, &info)
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let kind = msg
                .strip_prefix('[')
                .and_then(|rest| rest.split(']').next())
                .unwrap_or("network")
                .to_string();
            FormDrill {
                site_id: target.site_id.clone(),
                site_name: target.site_name.clone(),
                error_kind: Some(kind),
                error: Some(msg),
                ..Default::default()
            }
        }
    }
}

/// 供门禁/自动发种复用：按演练同一套逻辑分析「已构造好的上传字段」
pub fn analyze_prepared(
    site_id: &str,
    site_name: &str,
    prep: &PreparedUpload,
    info: &ReleaseInfo,
) -> FormDrill {
    analyze_form(site_id, site_name, prep, info)
}

/// 演练分析（纯函数，单测友好）
pub(super) fn analyze_form(
    site_id: &str,
    site_name: &str,
    prep: &PreparedUpload,
    info: &ReleaseInfo,
) -> FormDrill {
    let f = |k: &str| field_lookup(&prep.fields, k).unwrap_or("").to_string();
    let mut d = FormDrill {
        site_id: site_id.into(),
        site_name: site_name.into(),
        field_count: prep.fields.len(),
        select_count: select_names(&prep.form_html).len(),
        ..Default::default()
    };
    let title = if !f("name").is_empty() {
        f("name")
    } else {
        f("title")
    };
    d.title_set = title == info.name;
    // 无标题输入框（tjupt 实测）：标题由种子文件名自动生成，不算缺口
    d.title_auto =
        !d.title_set && !form_has(&prep.form_html, "name") && !form_has(&prep.form_html, "title");
    d.subtitle_set = f("small_descr") == info.subtitle || f("subtitle") == info.subtitle;
    // 简介：NexusPHP descr / Gazelle 系 release_desc
    let descr_text = ["descr", "release_desc", "description", "content"]
        .iter()
        .find_map(|k| {
            let v = f(k);
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        })
        .unwrap_or_default();
    d.descr_bytes = descr_text.len();
    for cand in ["technical_info", "media_info", "media_bdinfo", "mediainfo"] {
        if !f(cand).is_empty() {
            d.mediainfo_field = Some(cand.to_string());
            break;
        }
    }
    if d.mediainfo_field.is_none() && descr_text.contains("MediaInfo") {
        d.mediainfo_field = Some("内嵌简介".into());
    }
    for l in [
        "imdbid",
        "imdb_url",
        "url",
        "douban",
        "douban_id",
        "douban_url",
    ] {
        if !f(l).is_empty() {
            d.link_fields.push(l.to_string());
        }
    }
    d.picked_tags = prep.picked_tags.clone();
    d.tag_warn = prep.tag_warn.clone();
    d.notes = prep.select_notes.clone();
    // JS 驱动的自定义上传界面（hdcity 实测）：简介/MediaInfo 静态填不进
    let form = upload_form_block(&prep.form_html).unwrap_or(&prep.form_html);
    if js_driven_form(form) {
        d.notes.push(
            "该站上传界面是 JS 驱动（表单仅种子+隐藏字段），简介/MediaInfo 需上传后在网页里补"
                .into(),
        );
    }
    d.audit_panel = pre_audit_supported(&prep.form_html);
    d.action = Some(prep.action.clone());
    d.select_options = drill_select_options(&prep.form_html);
    // 只有页面没有 `type` 分类下拉、分类靠 JS 树/两级联动选的站（xingtan 实测 328 行），
    // 才把 mode_ 隐藏行整批排除；有 `type` 下拉的站（iloli/muxuege 的 4/6 分支）
    // 分支行仍按普通下拉校验，漏填要点名。
    let hidden_mode = if form_has(&prep.form_html, "type") {
        std::collections::HashSet::new()
    } else {
        hidden_mode_select_names(&prep.form_html)
    };
    let mut hidden_mode_n = 0usize;
    for name in select_names(&prep.form_html) {
        // BBCode 编辑器的 color/font/size 装饰下拉与发种完备性无关
        if matches!(name.as_str(), "color" | "font" | "size") {
            continue;
        }
        // 分类联动隐藏行（mode_*）：按所选分类由页面 JS 显隐，静态演练不计缺口
        if hidden_mode.contains(&name) {
            hidden_mode_n += 1;
            continue;
        }
        let v = f(&name);
        if v.is_empty() || v == "0" {
            d.open_selects.push(name);
        } else if name == "type" || name.contains("_sel") {
            let label = select_option_meta(&prep.form_html, &name, &v)
                .map(|(l, _)| l)
                .unwrap_or_else(|| v.clone());
            d.select_picks.push((name, label));
        }
    }
    if hidden_mode_n > 0 {
        d.notes.push(format!(
            "另有 {hidden_mode_n} 个分类联动行（mode_*）的下拉按所选分类生效，静态演练不计缺口"
        ));
    }
    // 硬缺口下拉：域相关 + 前缀折叠（多组分类分支只要一组填上就不算）；
    // 垂直站分类与样本不适配不算表单缺口
    const CORE_QUALITY: &[(&str, &[&str])] = &[
        ("medium_sel", MEDIUM_KEYWORDS),
        ("codec_sel", CODEC_KEYWORDS),
        ("standard_sel", STANDARD_KEYWORDS),
        ("audiocodec_sel", AUDIO_KEYWORDS),
    ];
    if d.open_selects.iter().any(|s| s == "type") {
        let opts = select_options(&prep.form_html, "type");
        if !type_niche_miss(&opts, info) {
            d.core_select_gaps.push("type".into());
        }
    }
    for (prefix, kws) in CORE_QUALITY {
        let hit = |n: &String| {
            n == prefix || (n.starts_with(prefix) && n[prefix.len()..].starts_with('['))
        };
        if !d.open_selects.iter().any(&hit) {
            continue;
        }
        if d.select_picks.iter().any(|(n, _)| hit(n)) {
            continue;
        }
        // 域无关的同名下拉（如书站 audiocodec_sel 实为电子书格式）不算质量缺口
        if d.open_selects.iter().any(|s| {
            hit(s) && {
                let opts = select_options(&prep.form_html, s);
                !opts.is_empty() && options_in_domain(&opts, kws)
            }
        }) {
            d.core_select_gaps.push((*prefix).to_string());
        }
    }
    d
}

// ---------------- 站点官方预审（LuckAudit pre-audit） ----------------

/// 官方预审的一条问题（level：error / warning / suspicious / …）
#[derive(Debug, Clone)]
pub struct AuditDetail {
    pub level: String,
    pub message: String,
    /// 扣分（正数）
    pub score: i64,
    pub suggestion: Option<String>,
}

impl AuditDetail {
    /// 人看的一行：❌ 消息（-8 分）→ 建议
    pub fn line(&self) -> String {
        let icon = match self.level.to_ascii_uppercase().as_str() {
            "ERROR" => "❌",
            "WARNING" => "⚠️",
            "SUSPICIOUS" => "🔍",
            _ => "·",
        };
        let mut s = format!("{icon} {}（-{} 分）", self.message, self.score);
        if let Some(a) = &self.suggestion {
            if !a.is_empty() {
                s.push_str(&format!(" → {a}"));
            }
        }
        s
    }
}

/// 站点官方预审结论（`POST /api/auto-audit/pre-audit`，luckaudit.js 同款协议）
#[derive(Debug, Clone)]
pub struct AuditVerdict {
    pub passed: bool,
    pub status: String,
    pub total_score: i64,
    pub error_count: i64,
    pub warning_count: i64,
    pub suspicious_count: i64,
    pub details: Vec<AuditDetail>,
    pub suggestions: Vec<String>,
}

impl AuditVerdict {
    /// 人看的一行摘要
    pub fn summary(&self) -> String {
        format!(
            "{}（总分 {}/100｜❌ {} ⚠️ {} 🔍 {}）",
            self.status,
            self.total_score,
            self.error_count,
            self.warning_count,
            self.suspicious_count
        )
    }
}

/// 上传页带 LuckAudit 面板（`luckaudit.js`）的站才有官方预审接口
pub fn pre_audit_supported(form_html: &str) -> bool {
    form_html.to_ascii_lowercase().contains("luckaudit")
}

/// 调站点官方预审核（LuckAudit）：把将提交的字段原样交给它打分。
/// 只应在 `pre_audit_supported` 为真时调用。
///
/// 传输走表单编码（`post_form`）而非 JSON：站点 Cloudflare WAF 拦 JSON POST，
/// 表单畅通且应用 `$request->input()` 能读 PHP 嵌套语法（`type[id]=…`）。
pub fn run_pre_audit(
    target: &NexusPhp,
    info: &ReleaseInfo,
    prep: &PreparedUpload,
) -> Result<AuditVerdict> {
    let payload = build_audit_payload(info, prep);
    let form = json_to_form(&payload);
    let url = resolve_url("/api/auto-audit/pre-audit", &target.base);
    let (status, body) = target
        .post_form(&url, &form, &prep.form_url)
        .context("调用官方预审接口失败")?;
    parse_audit_response(status, &body)
}

/// JSON 值摊平成 PHP 数组语法的表单字段对：
/// `{"type":{"id":"404"}}` → `type[id]=404`；`tags:[{id:23}]` → `tags[0][id]=23`。
/// null 与空对象/空数组不出字段（服务端把缺字段当「未选」处理，正是预审要判的）。
pub(super) fn json_to_form(v: &serde_json::Value) -> Vec<Field> {
    let mut out = Vec::new();
    flatten_form(v, "", &mut out);
    out
}

pub(super) fn flatten_form(v: &serde_json::Value, prefix: &str, out: &mut Vec<Field>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}[{k}]")
                };
                flatten_form(val, &key, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, val) in arr.iter().enumerate() {
                flatten_form(val, &format!("{prefix}[{i}]"), out);
            }
        }
        serde_json::Value::Null => {}
        serde_json::Value::String(s) => out.push((prefix.to_string(), s.clone())),
        other => out.push((prefix.to_string(), other.to_string())),
    }
}

pub(super) fn parse_audit_response(status: u16, body: &str) -> Result<AuditVerdict> {
    if !body.trim_start().starts_with('{') {
        let head: String = body.chars().take(80).collect();
        bail!("官方预审返回了非 JSON（HTTP {status}，可能登录态失效）：{head}");
    }
    let v: serde_json::Value = serde_json::from_str(body).context("官方预审响应解析失败")?;
    if v.get("success").and_then(|s| s.as_bool()) == Some(false) {
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
        let err = v.get("errors").map(|e| e.to_string()).unwrap_or_default();
        bail!("官方预审接口报错：{msg}{err}");
    }
    let data = v
        .get("data")
        .ok_or_else(|| anyhow!("官方预审响应缺 data 段"))?;
    let details = data
        .get("details")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .map(|d| AuditDetail {
                    level: d.get("level").and_then(|x| x.as_str()).unwrap_or("").into(),
                    message: d
                        .get("message")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .into(),
                    score: d.get("score").and_then(|x| x.as_i64()).unwrap_or(0),
                    suggestion: d
                        .get("suggestion")
                        .and_then(|x| x.as_str())
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    let suggestions = data
        .get("suggestions")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(AuditVerdict {
        passed: data
            .get("passed")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        status: data
            .get("status")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .into(),
        total_score: data.get("totalScore").and_then(|x| x.as_i64()).unwrap_or(0),
        error_count: data.get("errorCount").and_then(|x| x.as_i64()).unwrap_or(0),
        warning_count: data
            .get("warningCount")
            .and_then(|x| x.as_i64())
            .unwrap_or(0),
        suspicious_count: data
            .get("suspiciousCount")
            .and_then(|x| x.as_i64())
            .unwrap_or(0),
        details,
        suggestions,
    })
}

/// 官方预审载荷：与 luckaudit.js `collectTorrentInfo` 同构——如实反映将提交的字段
/// （简介含自动补的链接段、质量下拉含 `X_sel[n]` 下标形式）
pub(super) fn build_audit_payload(info: &ReleaseInfo, prep: &PreparedUpload) -> serde_json::Value {
    let f = |key: &str| -> String {
        field_lookup(&prep.fields, key)
            .unwrap_or_default()
            .to_string()
    };
    // 简介剥掉内联事件属性再送审：站点 WAF 对表单体里的 on* 事件属性计异常分
    // （实测 2 处放行、4 处即 403「网站防火墙」），NexusPHP 截图段天然带一串
    // onload/onclick；对预审打分毫无意义，只改预审载荷，真实上传一字不动。
    let description = strip_event_handlers(&f("descr"));
    let technical_info = ["technical_info", "media_info", "media_bdinfo", "mediainfo"]
        .iter()
        .map(|n| f(n))
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| info.mediainfo.clone());

    let mut external = serde_json::Map::new();
    if let Some(tt) = &info.imdb {
        external.insert(
            "imdb".into(),
            format!("https://www.imdb.com/title/{tt}/").into(),
        );
    }
    if let Some(db) = &info.douban {
        external.insert(
            "douban".into(),
            format!("https://movie.douban.com/subject/{db}/").into(),
        );
    }
    if let Some(tm) = &info.tmdb {
        external.insert("tmdb".into(), tm.clone().into());
    }

    // 分类：value 0/空 = 未选（luckaudit.js 同款判法）
    let type_val = f("type");
    let type_obj = if type_val.is_empty() || type_val == "0" {
        serde_json::Value::Null
    } else {
        let (label, class_name) =
            select_option_meta(&prep.form_html, "type", &type_val).unwrap_or_default();
        serde_json::json!({
            "id": type_val,
            "name": label,
            "mode": select_mode(&prep.form_html, "type").unwrap_or_default(),
            "class_name": class_name,
        })
    };

    // 质量：source/medium/codec/standard/processing/audiocodec/team（`X_sel` 或 `X_sel[n]`）
    let mut quality = serde_json::Map::new();
    for key in [
        "source",
        "medium",
        "codec",
        "standard",
        "processing",
        "audiocodec",
        "team",
    ] {
        let prefix = format!("{key}_sel");
        let Some((fname, val)) = prep.fields.iter().find(|(n, _)| {
            n == &prefix || (n.starts_with(&prefix) && n[prefix.len()..].starts_with('['))
        }) else {
            continue;
        };
        if val.is_empty() || val == "0" {
            continue;
        }
        let (label, _) = select_option_meta(&prep.form_html, fname, val).unwrap_or_default();
        quality.insert(
            key.to_string(),
            serde_json::json!({ "id": val, "name": label }),
        );
    }

    // 标签：value + 复选框文案
    let tags: Vec<serde_json::Value> = prep
        .picked_tags
        .iter()
        .map(|v| serde_json::json!({ "id": v, "name": checkbox_label(&prep.form_html, v) }))
        .collect();

    let now = iso_utc(now_unix());
    serde_json::json!({
        "name": info.name.trim(),
        "small_descr": info.subtitle.trim(),
        "pt_gen_url": f("pt_gen"),
        "imdb_url": external.get("imdb").cloned().unwrap_or_default(),
        "external_links": external,
        "description": description.trim(),
        "technical_info": technical_info.trim(),
        "type": type_obj,
        "quality": quality,
        "tags": tags,
        "collected_at": now,
        "export_time": now,
        "page_url": prep.form_url,
        "user_info": extract_luck_audit_user(&prep.form_html)
            .unwrap_or(serde_json::Value::Null),
    })
}

/// 剥掉 HTML 标签内的内联事件属性（`onload="…"` / `onclick='…'`，大小写不敏感）。
/// 只动标签内部，正文里出现的 "on=…" 字样不受影响；多字节 UTF-8 原样保留。
/// 站点 WAF 对表单体里的 `on*=` 按出现次数异常计分（实测 2 处放行、4 处即
/// 403「网站防火墙」），NexusPHP 简介截图段天然带一串——对预审打分无意义。
pub(super) fn strip_event_handlers(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut seg_start = 0;
    let mut in_tag = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => in_tag = true,
            b'>' => in_tag = false,
            _ => {}
        }
        // 属性名起笔：标签内、空白后的 onXxx=（至少 2 个字母，避免误伤 on=）
        if in_tag
            && (bytes[i] | 0x20) == b'o'
            && i > 0
            && bytes[i - 1].is_ascii_whitespace()
            && bytes.get(i + 1).map(|b| b | 0x20) == Some(b'n')
        {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                j += 1;
            }
            if j >= i + 4 && bytes.get(j) == Some(&b'=') {
                j += 1;
                match bytes.get(j) {
                    Some(&q @ (b'"' | b'\'')) => {
                        j += 1;
                        while j < bytes.len() && bytes[j] != q {
                            j += 1;
                        }
                        if j < bytes.len() {
                            j += 1;
                        }
                    }
                    _ => {
                        while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'>'
                        {
                            j += 1;
                        }
                    }
                }
                // 连同属性前的空格一起吞掉，不留双空格
                let mut seg_end = i;
                while seg_end > seg_start && bytes[seg_end - 1] == b' ' {
                    seg_end -= 1;
                }
                out.push_str(&html[seg_start..seg_end]);
                seg_start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&html[seg_start..]);
    out
}

/// 字段查找：精确名，或 `key[...]` 下标形式（medium_sel[4]）
pub(super) fn field_lookup<'a>(fields: &'a [Field], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(n, _)| n == key || (n.starts_with(key) && n[key.len()..].starts_with('[')))
        .map(|(_, v)| v.as_str())
}

/// `<select name=field>` 里指定 value 的选项文案与 class-name（同名多块全找）
pub(super) fn select_option_meta(page: &str, field: &str, value: &str) -> Option<(String, String)> {
    for block in select_blocks(page, field) {
        let mut rest = block;
        while let Some(p) = rest.to_ascii_lowercase().find("<option") {
            let after = &rest[p..];
            let end = after.find('>').map(|j| j + 1).unwrap_or(after.len());
            let tag = &after[..end];
            if attr(tag, "value").as_deref() == Some(value) {
                let text_end = after[end..]
                    .find('<')
                    .map(|i| end + i)
                    .unwrap_or(after.len());
                let label = strip_tags(&after[end..text_end]).trim().to_string();
                let class_name = attr(tag, "data-class-name").unwrap_or_default();
                return Some((label, class_name));
            }
            rest = &after[end..];
        }
    }
    None
}

/// `<select name=field>` 的 data-mode 属性（分类联动模式）
pub(super) fn select_mode(page: &str, field: &str) -> Option<String> {
    let block = select_block(page, field)?;
    let end = block.find('>')? + 1;
    attr(&block[..end], "data-mode")
}

/// 取 `<select name=field>` 整块（含开闭标签内文）；同名多块取第一块
pub(super) fn select_block<'a>(page: &'a str, field: &str) -> Option<&'a str> {
    select_blocks(page, field).into_iter().next()
}

/// 同名 `<select>` 的全部块（分类分支的第二套下拉与首套同名）
pub(super) fn select_blocks<'a>(page: &'a str, field: &str) -> Vec<&'a str> {
    let lower = page.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<select") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        if attr(&page[start..end], "name").as_deref() == Some(field) {
            let close = lower[start..]
                .find("</select>")
                .map(|j| start + j)
                .unwrap_or(page.len());
            out.push(&page[start..close]);
        }
        i = end;
    }
    out
}

/// 复选框 value 对应的文案：input 标签后到下一个 `<` 之前（包装标签时退 80 字符窗口）
pub(super) fn checkbox_label(page: &str, value: &str) -> String {
    let lower = page.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = lower[i..].find("<input") {
        let start = i + p;
        let end = lower[start..]
            .find('>')
            .map(|j| start + j + 1)
            .unwrap_or(page.len());
        let tag = &page[start..end];
        if tag.to_ascii_lowercase().contains("checkbox")
            && attr(tag, "value").as_deref() == Some(value)
        {
            let after = &page[end..floor_char_boundary(page, end + 80)];
            // 先取到下一个标签为止的纯文本；input 后直接跟标签（<label>…）再退窗口。
            // 下一个 <input 处截断，密集标签网格不误配邻格
            let next_input = after
                .to_ascii_lowercase()
                .find("<input")
                .unwrap_or(after.len());
            let cut = after[..next_input].find('<').unwrap_or(next_input);
            let inline = strip_tags(&after[..cut]).trim().to_string();
            if !inline.is_empty() {
                return inline;
            }
            return strip_tags(&after[..next_input]).trim().to_string();
        }
        i = end;
    }
    String::new()
}

/// 页面内嵌 `window.LUCK_AUDIT_USER = {...}`（官方预审 user_info 段）
pub(super) fn extract_luck_audit_user(form_html: &str) -> Option<serde_json::Value> {
    let p = form_html.find("LUCK_AUDIT_USER")?;
    let brace = form_html[p..].find('{')? + p;
    let b = form_html.as_bytes();
    let mut depth = 0usize;
    let mut i = brace;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&form_html[brace..=i]).ok();
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// unix 秒 → `2026-11-18T07:50:00.000Z`（预审载荷时间戳；民用历换算不引 chrono）
pub(super) fn iso_utc(unix: i64) -> String {
    let secs = unix.rem_euclid(86400);
    let days = unix.div_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}
