//! `reseed` 的单元测试；从 `reseed.rs` 拆出，便于主流程文件收敛。

use super::*;

#[test]
fn imdb_search_ignores_sidebar_title_without_name_match() {
    let base = Url::parse("https://13city.org/").unwrap();
    let html = r#"
<a title="Lao hu li AKA Old Fox 2023 1080p TWN Blu-ray AVC DTS-HD MA 7.1-CMCT" href="details.php?id=47197">Old Fox</a>
<a title="Agatha Christies Poirot S01-S13 Collection 1080p Bluray x265 10bit MNHD-FRDS" href="details.php?id=18000">Poirot</a>"#;
    let hit = first_imdb_hit(
        html,
        &base,
        "Agatha Christies Poirot S01-S13 Collection 1080p Bluray x265 10bit MNHD-FRDS",
    )
    .expect("应取到名称匹配的 Poirot");
    assert_eq!(hit.0, "https://13city.org/details.php?id=18000");
    assert!(first_imdb_hit(html, &base, "Some.Other.Movie.2024.1080p.BluRay.x265-GRP").is_none());
}

const PAGE: &str = r#"
<html><head><title>x</title></head><body>
<h1>The.Disguiser.Complete.HDTV.1080p.AAC.x264-JING</h1>
<table><tr><td>副标题</td><td>伪装者 全41集</td></tr></table>
<div id="kdescr">
<p>IMDb: https://www.imdb.com/title/tt4823720/</p>
<p>豆瓣: https://movie.douban.com/subject/26302614/</p>
<pre>Unique ID : 123
Format : AVC
Bit rate : 3500 Kbps
Width : 1920
Height : 1080
Frame rate : 25</pre>
</div>
</body></html>
"#;

#[test]
fn parses_release_page() {
    let info = parse_release_page(PAGE, "fallback");
    assert_eq!(info.name, "The.Disguiser.1080p.HDTV.x264.AAC-JING");
    assert_eq!(info.subtitle, "伪装者 全41集");
    assert!(info.mediainfo.contains("Unique ID"));
    assert_eq!(info.imdb.as_deref(), Some("tt4823720"));
    assert_eq!(info.douban.as_deref(), Some("26302614"));
}

#[test]
fn exist_marker_accepts_existed_redirect() {
    let body = format!("<html>{}</html>", "x".repeat(30_000));
    let m = exist_marker("https://sbpt.link/details.php?id=21692&existed=1", &body);
    assert_eq!(m, Some("跳转 URL 带 existed=1"));
}

#[test]
fn exist_marker_ignores_rerendered_upload_form() {
    // 必填项校验失败会把 upload.php 原样返回，页面帮助文案里可能带「已存在」
    let body = r#"<form action="takeupload.php" method="post" enctype="multipart/form-data">
<p>请先搜索站内是否已有该资源；重复资源会提示种子已存在。</p></form>"#;
    assert_eq!(exist_marker("https://x/upload.php", body), None);
}

#[test]
fn exist_marker_ignores_long_page_boilerplate() {
    let mut body = String::from("<html><body>");
    body.push_str(&"y".repeat(12_000));
    body.push_str("<p>规则：重复上传会提示已存在</p></body></html>");
    assert_eq!(exist_marker("https://x/details.php?id=1", &body), None);
}

#[test]
fn exist_marker_reads_errorbox_even_when_form_is_rerendered() {
    // pthome 实测：重复上传会把错误框和上传表单一起返回
    let body = r#"<form action="takeupload.php" method="post" enctype="multipart/form-data">
<div id="errorbox">该种子已存在！</div></form>"#;
    assert_eq!(
        exist_marker("https://x/takeupload.php", body),
        Some("已存在")
    );
}

#[test]
fn exist_marker_reads_exact_notice_even_when_form_is_rerendered() {
    // 有的模板把「该种子已存在！」写在普通提示区，不是 errorbox；
    // 页面仍带 multipart 上传表单，不能因此丢掉重复结论。
    let body = r#"<form action="takeupload.php" method="post" enctype="multipart/form-data">
<div class="notice">该种子已存在！</div></form>"#;
    assert_eq!(
        exist_marker("https://x/takeupload.php", body),
        Some("该种子已存在！")
    );
    // 帮助文案里的泛化「提示种子已存在」不算重复。
    let boilerplate = r#"<form action="takeupload.php" method="post" enctype="multipart/form-data">
<p>请先搜索站内是否已有该资源；重复资源会提示种子已存在。</p></form>"#;
    assert_eq!(exist_marker("https://x/takeupload.php", boilerplate), None);
    // 普通长页面（>8KB，没有重填表单）只写这一句，也要认重复。
    let long_notice = format!(
        "<html><body>{}<div>该种子已存在！</div></body></html>",
        " ".repeat(9000)
    );
    assert_eq!(
        exist_marker("https://x/takeupload.php", &long_notice),
        Some("该种子已存在！")
    );
    // 只写“已存在！”的长页面同样要认，不依赖“该种子”前缀。
    let plain_long = format!(
        "<html><body>{}<div>已存在！</div></body></html>",
        " ".repeat(9000)
    );
    assert_eq!(
        exist_marker("https://x/takeupload.php", &plain_long),
        Some("已存在！")
    );
}

#[test]
fn exist_marker_reads_short_notice() {
    let body = "<html><body><p>该种子已存在，请勿重复上传</p></body></html>";
    assert_eq!(
        exist_marker("https://x/takeupload.php", body),
        Some("已存在")
    );
}

#[test]
fn preflight_blocks_empty_mediainfo() {
    let mut info = ReleaseInfo {
        name: "x".into(),
        descr: "IMDb: https://www.imdb.com/title/tt1/".into(),
        mediainfo: String::new(),
        ..Default::default()
    };
    let pf = preflight(
        &mut info,
        &[],
        &crate::gates::SiteRules::default(),
        "luckpt",
    );
    assert!(!pf.ok());
    assert!(pf.errors.iter().any(|e| e.contains("MediaInfo")));
}

#[test]
fn preflight_autofixes_links() {
    let mut info = ReleaseInfo {
        name: "x 中字".into(),
        descr: "简介没有链接".into(),
        mediainfo: "General\nDuration : 1h\nVideo\nWidth : 1920\nHeight : 1080".into(),
        imdb: Some("tt4823720".into()),
        ..Default::default()
    };
    let tags = vec!["中字".to_string()];
    let pf = preflight(
        &mut info,
        &tags,
        &crate::gates::SiteRules::default(),
        "luckpt",
    );
    assert!(pf.ok(), "{:?}", pf.errors);
    assert!(info.descr.contains("imdb.com"));
    assert!(!pf.fixes.is_empty());
}

#[test]
fn preflight_relaxes_mediainfo_when_rules_allow() {
    // requireMediaInfo=false：预检不再阻断，降为警告（否则门禁放行也发不出去）
    let mut info = ReleaseInfo {
        name: "x".into(),
        descr: "IMDb: https://www.imdb.com/title/tt1/".into(),
        mediainfo: String::new(),
        ..Default::default()
    };
    let rules = crate::gates::SiteRules {
        require_mediainfo: Some(false),
        ..Default::default()
    };
    let pf = preflight(&mut info, &[], &rules, "luckpt");
    assert!(
        pf.ok(),
        "requireMediaInfo=false 时不应阻断：{:?}",
        pf.errors
    );
    assert!(pf.warnings.iter().any(|w| w.contains("放宽")));
}

#[test]
fn parses_form_fields() {
    let form = r#"<form action="takeupload.php" method="post">
<input type="hidden" name="passkey" value="abc">
<input type="text" name="name" value="">
<input type="text" name="small_descr" value="">
<select name="type"><option value="1">电影</option><option value="2" selected>剧集</option></select>
<input type="checkbox" name="tags[]" value="cnsub"> 中字
</form>"#;
    let fields = parse_form_fields(form);
    assert!(fields.contains(&("passkey".into(), "abc".into())));
    assert!(fields.contains(&("type".into(), "2".into())));
    let (fs, picked, _notes) = build_upload_fields(
        &ReleaseInfo {
            name: "T".into(),
            subtitle: "s".into(),
            descr: "d".into(),
            mediainfo: "m".into(),
            ..Default::default()
        },
        form,
        &["中字"],
    );
    assert!(fs.contains(&("name".into(), "T".into())));
    assert!(fs.contains(&("small_descr".into(), "s".into())));
    assert!(picked.iter().any(|p| p == "cnsub"));
}

fn tjupt_series_rule() -> crate::gates::CategoryRule {
    crate::gates::category_rules_default("tjupt")
        .into_iter()
        .next()
        .expect("tjupt 应有剧集分类规则")
}

#[test]
fn category_rule_missing_control_blocks_and_names_field() {
    // 片段里没有 cname 控件 → 记下缺口点名 cname，不臆造字段名
    let fragment = r#"<form>
<input type="text" name="ename">
<select name="specificcat"><option>其他</option></select>
</form>"#;
    let info = ReleaseInfo {
        name: "Poirot S01 1080p x265 MNHD".into(),
        subtitle: "【大侦探波洛】国英双配".into(),
        descr: "英剧 推理".into(),
        ..Default::default()
    };
    let mut fields = vec![("type".into(), "402".into())];
    let mut gaps = Vec::new();
    apply_category_rule_fields(
        &mut fields,
        &mut gaps,
        &info,
        fragment,
        &tjupt_series_rule(),
    );
    assert!(gaps.iter().any(|g| g.contains("cname")), "{gaps:?}");
    assert!(fields.iter().all(|(n, _)| n != "cname"), "{fields:?}");
    // 其余两个控件在片段里，照常填上
    assert!(fields.iter().any(|(n, _)| n == "ename"));
    assert!(fields.iter().any(|(n, _)| n == "specificcat"));
}

#[test]
fn category_rule_all_controls_present_fills_and_passes() {
    let fragment = r#"<form>
<input type="text" name="cname">
<input type="text" name="ename">
<select name="specificcat"><option>其他</option><option>英剧</option></select>
</form>"#;
    let info = ReleaseInfo {
        name: "Agatha Christies Poirot S01-S13 Collection Bluray 1080p x265 10bit MNHD-FRDS".into(),
        subtitle: "【大侦探波洛】国英双配 外挂中英ass字幕 10bit HEVC版本".into(),
        descr: "◎标　　签　英剧 | 推理".into(),
        ..Default::default()
    };
    let mut fields = vec![("type".into(), "402".into())];
    let mut gaps = Vec::new();
    apply_category_rule_fields(
        &mut fields,
        &mut gaps,
        &info,
        fragment,
        &tjupt_series_rule(),
    );
    assert!(gaps.is_empty(), "{gaps:?}");
    assert!(fields.contains(&("cname".into(), "大侦探波洛".into())));
    assert!(fields.contains(&("ename".into(), info.name.clone())));
    assert!(fields.contains(&("specificcat".into(), "英剧".into())));
}

#[test]
fn category_rule_no_chinese_name_goes_manual() {
    // 副标题/简介都抽不出中文 → 落人工，不发空 cname
    let fragment = r#"<form>
<input type="text" name="cname">
<input type="text" name="ename">
<select name="specificcat"><option>其他</option></select>
</form>"#;
    let info = ReleaseInfo {
        name: "Some English Title 2024 1080p WEB-DL x264".into(),
        subtitle: "Some English Subtitle".into(),
        descr: "IMDb: https://www.imdb.com/title/tt1/\nEnglish description".into(),
        ..Default::default()
    };
    let mut fields = vec![("type".into(), "402".into())];
    let mut gaps = Vec::new();
    apply_category_rule_fields(
        &mut fields,
        &mut gaps,
        &info,
        fragment,
        &tjupt_series_rule(),
    );
    assert!(
        gaps.iter()
            .any(|g| g.contains("cname") && g.contains("派生不出中文名")),
        "{gaps:?}"
    );
    assert!(fields.iter().all(|(n, _)| n != "cname"), "{fields:?}");
}

#[test]
fn category_rule_empty_english_name_goes_manual() {
    let fragment = r#"<form>
<input type="text" name="cname">
<input type="text" name="ename">
<select name="specificcat"><option>其他</option></select>
</form>"#;
    let info = ReleaseInfo {
        name: "".into(),
        subtitle: "【伪装者】国产剧".into(),
        descr: "大陆 谍战".into(),
        ..Default::default()
    };
    let mut fields = vec![("type".into(), "402".into())];
    let mut gaps = Vec::new();
    apply_category_rule_fields(
        &mut fields,
        &mut gaps,
        &info,
        fragment,
        &tjupt_series_rule(),
    );
    assert!(
        gaps.iter()
            .any(|g| g.contains("ename") && g.contains("英文名")),
        "{gaps:?}"
    );
    assert!(fields.iter().all(|(n, _)| n != "ename"), "{fields:?}");
}

#[test]
fn ubits_quality_fields_get_region_and_team_defaults() {
    let form = r#"<form action="takeupload.php" method="post">
<select name="source_sel[4]"><option value="0" selected>请选择一项</option><option value="1">中国大陆</option></select>
<select name="team_sel[4]"><option value="0" selected>请选择一项</option><option value="5">Other</option></select>
<select name="medium_sel[4]"><option value="4" selected>WEB-DL</option></select>
<input type="text" name="name" value="">
<textarea name="descr"></textarea>
</form>"#;
    let info = ReleaseInfo {
        name: "The Story Of Ming Lan 2018 WEB-DL 4k H265 60fps 10bit AAC-HDSWEB".into(),
        subtitle: "知否知否应是绿肥红瘦".into(),
        descr: "中国大陆 古装".into(),
        ..Default::default()
    };
    let (fields, _, _) = build_upload_fields_for_site(&info, form, &[], "ubits");
    assert!(fields.contains(&("source_sel[4]".into(), "1".into())));
    assert!(fields.contains(&("team_sel[4]".into(), "5".into())));
}

#[test]
fn lajidui_quality_fields_fill_region_container_and_team() {
    let form = r#"<form action="takeupload.php" method="post">
<select name="source_sel[4]"><option value="0" selected>请选择一项</option><option value="7">大陆</option></select>
<select name="processing_sel[4]"><option value="0" selected>请选择一项</option><option value="10">MKV</option><option value="11">MP4</option></select>
<select name="team_sel[4]"><option value="0" selected>请选择一项</option><option value="13">QHstudIo</option><option value="5">Other</option></select>
<input type="text" name="name" value="">
<textarea name="descr"></textarea>
</form>"#;
    let info = ReleaseInfo {
        name: "In the Name of People 2017 S01 2160p 60fps WEB-DL HEVC 10bit AAC-QHstudIo".into(),
        subtitle: "人民的名义 全55集".into(),
        descr: "中国大陆 2017".into(),
        mediainfo: "Complete name : K:\\Download\\E01.mp4\nFormat : MPEG-4".into(),
        ..Default::default()
    };
    let (fields, _, _) = build_upload_fields_for_site(&info, form, &[], "lajidui");
    assert!(fields.contains(&("source_sel[4]".into(), "7".into())));
    assert!(fields.contains(&("processing_sel[4]".into(), "11".into())));
    assert!(fields.contains(&("team_sel[4]".into(), "13".into())));
}

#[test]
fn lajidui_unknown_team_uses_other_and_mkv() {
    let form = r#"<form action="takeupload.php" method="post">
<select name="source_sel[4]"><option value="0" selected>请选择一项</option><option value="7">大陆</option></select>
<select name="processing_sel[4]"><option value="0" selected>请选择一项</option><option value="10">MKV</option><option value="11">MP4</option></select>
<select name="team_sel[4]"><option value="0" selected>请选择一项</option><option value="5">Other</option></select>
<input type="text" name="name" value="">
<textarea name="descr"></textarea>
</form>"#;
    let info = ReleaseInfo {
        name: "The Disguiser 1080p HDTV x264 AAC-JING".into(),
        subtitle: "伪装者".into(),
        descr: "中国大陆".into(),
        mediainfo: "Complete name : J:\\Download\\EP01.mkv\nFormat : Matroska".into(),
        ..Default::default()
    };
    let (fields, _, _) = build_upload_fields_for_site(&info, form, &[], "lajidui");
    assert!(fields.contains(&("processing_sel[4]".into(), "10".into())));
    assert!(fields.contains(&("team_sel[4]".into(), "5".into())));
}

#[test]
fn bracket_name_handles_multibyte_brackets() {
    assert_eq!(
        first_bracket_name("【大侦探波洛】国英双配"),
        Some("大侦探波洛".to_string())
    );
    assert_eq!(first_bracket_name("no brackets"), None);
}

#[test]
fn chinese_subs_detection() {
    let mut info = ReleaseInfo {
        name: "Show.S01.1080p".into(),
        ..Default::default()
    };
    assert!(!info.has_chinese_subs());
    info.subtitle = "内封简体中文".into();
    assert!(info.has_chinese_subs());
}

#[test]
fn mediainfo_block_detection() {
    let good = "Unique ID : 1\nFormat : AVC\nBit rate : 3500\nWidth : 1920\nHeight : 1080";
    assert!(is_mediainfo_block(good));
    assert!(!is_mediainfo_block(
        "随便一段话\n第二行\n第三行\n第四行\n第五行"
    ));
}

#[test]
fn empty_upload_response_is_reported_as_timeout_not_unknown() {
    let msg = extract_upload_error("", 200);
    assert!(
        msg.contains("空回包") && msg.contains("未确认发布"),
        "{msg}"
    );
    assert!(!msg.contains("未识别的返回"), "{msg}");
}

#[test]
fn http_200_unrecognized_response_is_unconfirmed_not_a_failure() {
    // city13/hdvideo 这类站会回 HTTP 200，但页面既不是详情页也不是错误框。
    // 这种回包不能直接判失败，先交给站内检索核对；403/空回包仍有明确含义。
    let body = "<html><body><h2>页面没有可识别的成功或失败结构</h2></body></html>";
    let msg = extract_upload_error(body, 200);
    assert_eq!(msg, "HTTP 200 未识别的返回");
    assert!(is_unconfirmed_http_200(&msg));
    assert!(is_unconfirmed_http_200(&format!(
        "{msg}；回包已存 /tmp/bsend-upload-city13-1.html"
    )));
    assert!(!is_unconfirmed_http_200("HTTP 403 未识别的返回"));
    assert!(!is_unconfirmed_http_200(
        "HTTP 200 空回包（站点后端没有返回内容，通常是处理超时或崩溃；本次未确认发布）"
    ));
}

#[test]
fn error_extraction() {
    let body = r#"<div id="errorbox">请填写必填项目</div>"#;
    assert_eq!(extract_upload_error(body, 200), "请填写必填项目");
    assert!(looks_like_lost_post("请填写必填项目"));
}

#[test]
fn error_extraction_does_not_split_utf8() {
    let body = format!("xx上传失败{}", "子".repeat(200));
    let msg = extract_upload_error(&body, 200);
    assert!(msg.contains("上传失败"), "{msg}");
    assert!(!site_rejects_size(&msg), "{msg}");
}

#[test]
fn missing_field_rejection_is_labeled_separately_from_timeout() {
    // tjupt 编辑页实测回“有项目没有填写”：是站点明确的缺字段拒绝，
    // 不能和后端超时/空回包混成一类，也不能只丢一句笼统“上传失败”。
    let body =
        r#"<html><body><div class="error">有项目没有填写，请检查后重新提交</div></body></html>"#;
    let msg = extract_upload_error(body, 200);
    assert!(msg.contains("缺少必填项"), "{msg}");
    assert!(msg.contains("有项目没有填写"), "{msg}");
    assert!(!msg.contains("空回包"), "{msg}");
    assert!(!msg.contains("未识别的返回"), "{msg}");
}

#[test]
fn tag_only_empty_shell_counts_as_empty_response_timeout() {
    // 正文只剩 HTML 标签、没有可见文本：按后端超时/空回包记，不算站点拒绝
    let body = r#"<html><head><title></title></head><body><form></form></body></html>"#;
    let msg = extract_upload_error(body, 200);
    assert!(msg.contains("空回包"), "{msg}");
    assert!(!msg.contains("未识别的返回"), "{msg}");
}

#[test]
fn too_large_torrent_is_a_site_limit_not_an_unknown_response() {
    // qingwa/CarPT 的真实回包：不是错误框，而是一个自动跳 offers.php 的表单
    let body = r#"<form id='torrent_too_large' action='/offers.php?add_offer=1&torrent_too_large=1' method='POST'>
            <input type='hidden' name='name' value='X' />
        </form>"#;
    let msg = extract_upload_error(body, 200);
    assert!(
        msg.contains("种子过大") && msg.contains("offers.php"),
        "{msg}"
    );
    assert!(!msg.contains("未识别的返回"), "{msg}");
    assert!(site_rejects_size(&msg), "{msg}");

    // 这类结果不是“已跳过”，要留成待跟进：审计词单独一个，周报能数出来
    let outcome = ReseedOutcome::Duplicate(DuplicateOutcome {
        mounted: false,
        status: CoverageStatus::PendingOffers,
        url: None,
        note: msg,
    });
    assert_eq!(outcome.audit_result(), "pending_offers");
}

#[test]
fn upload_failure_reason_after_h1_is_preserved() {
    // UBits 实测：标题也写“上传失败”，真正原因在 h1 后面的 p 里。
    let body = r#"<html><head><title>UBits :: 上传失败！ - Powered by NexusPHP</title></head>
<body><div class="main"></div><h1>上传失败！</h1>
<p>检测到种子简介中包含黑名单图床,请将图片重新上传至其他图床(例如 https://pixhost.to/)</p>
</td></tr></table></body></html>"#;
    let msg = extract_upload_error(body, 200);
    assert!(
        msg.contains("黑名单图床"),
        "应保留 h1 后面的真实原因，实际：{msg}"
    );
    assert!(msg.starts_with("站点拒绝："), "{msg}");
}

#[test]
fn generic_upload_error_dumps_body_to_temp_and_appends_path() {
    // iloli 实测那类通用错误框：没有可识别的错误框 class，只剩一句笼统「上传失败」。
    let body = r#"<html><head><title>上传失败！ - Powered by NexusPHP</title></head>
<body><p>上传失败！</p></body></html>"#;
    let msg = extract_upload_error(body, 200);
    assert!(is_generic_upload_error(&msg), "{msg}");

    let augmented = augment_upload_error_with_dump("iloli", body, &msg);
    assert!(augmented.contains("回包已存"), "{augmented}");

    let after = augmented.split("回包已存 ").nth(1).unwrap();
    let path: String = after.split('；').next().unwrap().trim().to_string();
    let file = PathBuf::from(&path);
    assert!(file.exists(), "落盘文件不存在：{path}");
    let saved = std::fs::read_to_string(&file).expect("落盘文件应可读");
    assert_eq!(saved, body);
    assert!(augmented.contains("上传失败"), "{augmented}");

    let _ = std::fs::remove_file(&file);
}

#[test]
fn concrete_upload_error_is_not_dumped_and_message_unchanged() {
    // 站点已给出明确拒因（缺必填项）：不走落盘分支，文案与改动前一致。
    let body =
        r#"<html><body><div class="error">有项目没有填写，请检查后重新提交</div></body></html>"#;
    let msg = extract_upload_error(body, 200);
    assert!(!is_generic_upload_error(&msg), "{msg}");
    let augmented = augment_upload_error_with_dump("iloli", body, &msg);
    assert_eq!(augmented, msg);
    assert!(!augmented.contains("回包已存"));
}

#[test]
fn errorbox_by_class_is_recognized_like_id() {
    let body = r#"<div class="errorbox">请填写必填项目</div>"#;
    assert_eq!(extract_upload_error(body, 200), "请填写必填项目");
}

#[test]
fn find_links() {
    assert_eq!(find_tt("xx tt4823720 yy"), Some("tt4823720".into()));
    assert_eq!(
        find_douban("https://movie.douban.com/subject/26302614/"),
        Some("26302614".into())
    );
    assert_eq!(
        find_tmdb("https://www.themoviedb.org/tv/61888"),
        Some("https://www.themoviedb.org/tv/61888".into())
    );
}

#[test]
fn strips_promo_suffix_not_content_tags() {
    assert_eq!(
        strip_promo_suffix("Show.S01 1080p-JING    [50%]"),
        "Show.S01 1080p-JING"
    );
    assert_eq!(strip_promo_suffix("Movie.2024 [FREE]"), "Movie.2024");
    assert_eq!(strip_promo_suffix("Movie.2024 [2x]"), "Movie.2024");
    assert_eq!(strip_promo_suffix("Movie.2024 [免费]"), "Movie.2024");
    assert_eq!(
        strip_promo_suffix("Movie.2024 [2X免費] 剩餘時間：2天23時"),
        "Movie.2024"
    );
    assert_eq!(strip_promo_suffix("Movie.2024 [未审]"), "Movie.2024");
    assert_eq!(strip_promo_suffix("Movie.2024 [未审核]"), "Movie.2024");
    // 内容标签保留
    assert_eq!(strip_promo_suffix("Movie.2024 [中字]"), "Movie.2024 [中字]");
    assert_eq!(
        strip_promo_suffix("Movie.2024 [中字] [50%]"),
        "Movie.2024 [中字]"
    );
    // 角标后面还跟着审核状态，不能只认“以 ] 结尾”：Liar Game 源页实测
    assert_eq!(
        strip_promo_suffix("Liar Game S01 2007 2160p WEB-DL H.264 AAC-ADWeb [50%] (通过)"),
        "Liar Game S01 2007 2160p WEB-DL H.264 AAC-ADWeb"
    );
}

#[test]
fn strips_free_promo_countdown_tail() {
    // luckpt 实测 h1：[免费] 后面还挂着倒计时，角标不在尾部
    assert_eq!(
        strip_promo_suffix(
            "Die Welle 2008 FRA BluRay 1080p x264 DTS-CMCT    [免费] 剩余时间：6天18时"
        ),
        "Die Welle 2008 FRA BluRay 1080p x264 DTS-CMCT"
    );
    assert_eq!(
        strip_promo_suffix("Show.S01 1080p-LuckAni [免费] 剩余时间：23时37分钟"),
        "Show.S01 1080p-LuckAni"
    );
    assert_eq!(
        strip_promo_suffix("Movie.2024 [50%] 剩餘時間：2天3小時"),
        "Movie.2024"
    );
    // hdvideo 实测：倒计时按年/月计，前面挂着百分比角标
    assert_eq!(
        strip_promo_suffix(
            "Star.Wars.Episode.IV.1977.1080p.BluRay.x264.MNHDR-FRDS    [50%] 剩余时间：26年4月"
        ),
        "Star.Wars.Episode.IV.1977.1080p.BluRay.x264.MNHDR-FRDS"
    );
    assert_eq!(strip_promo_suffix("Movie.2024 [免費]"), "Movie.2024");
    // 正文里提到「剩余时间」但后面不是倒计时：不动
    assert_eq!(
        strip_promo_suffix("Movie.2024 剩余时间剧情解析"),
        "Movie.2024 剩余时间剧情解析"
    );
}

#[test]
fn escaped_descr_is_decoded_once() {
    // luckpt 实测：kdescr 存的是转义一层的 HTML，站点只补 nl2br 的裸 <br />
    let escaped = "&lt;fieldset&gt;&lt;legend&gt; 引用 &lt;/legend&gt;&lt;b&gt;官组作品&lt;/b&gt;&lt;/fieldset&gt;&lt;br /&gt;<br />\n&lt;img src=&quot;https://x/a.jpg&quot;&gt;<br />\n&lt;span style=&quot;color:blue&quot;&gt;谢谢&lt;/span&gt;<br />";
    let out = unescape_descr(escaped);
    assert!(
        out.starts_with("<fieldset><legend> 引用 </legend>"),
        "{out}"
    );
    assert!(out.contains("<img src=\"https://x/a.jpg\">"), "{out}");
    // 双重转义的引号只解一层：&amp;quot; → &quot;（渲染成引号），不是裸引号
    assert_eq!(
        unescape_descr(
            "&lt;b&gt;&amp;quot;名&amp;quot;&lt;/b&gt;&lt;i&gt;x&lt;/i&gt;&lt;u&gt;y&lt;/u&gt;"
        ),
        "<b>&quot;名&quot;</b><i>x</i><u>y</u>"
    );
    // 正常裸 HTML 简介：原样保留
    let raw = "<fieldset><legend> 引用 </legend>正文</fieldset><img src=\"https://x/a.jpg\"><br />";
    assert_eq!(unescape_descr(raw), raw);
    // 纯文本里引用少量 HTML 示例（裸标签只有 nl2br）：不解码
    let text = "用 &lt;b&gt; 加粗，&lt;i&gt; 斜体，&lt;u&gt; 下划线<br /><br />就这样";
    assert_eq!(unescape_descr(text), text);
}

#[test]
fn escaped_descr_decodes_mixed_and_double_escaped_blocks() {
    // hdvideo 实测：裸 <div> 壳 + 转义 <fieldset> 混排，旧逻辑因裸标签多而整段不解码
    let mixed = r#"<div><fieldset><legend>引用</legend>正文</fieldset></div>
&lt;fieldset&gt;&lt;legend&gt; 引用 &lt;/legend&gt;&lt;br /&gt;转自&lt;b&gt;TLF&lt;/b&gt;&lt;/fieldset&gt;<br />"#;
    let out = unescape_descr(mixed);
    assert!(out.starts_with("<div><fieldset>"), "{out}");
    assert!(out.contains("<fieldset><legend> 引用 </legend>"), "{out}");
    assert!(!out.contains("&lt;fieldset"), "{out}");
    // 两层转义：&amp;lt;fieldset → &lt;fieldset → <fieldset
    let double = "&amp;lt;fieldset&amp;gt;&amp;lt;legend&amp;gt;引用&amp;lt;/legend&amp;gt;正文&amp;lt;/fieldset&amp;gt;";
    let out = unescape_descr(double);
    assert_eq!(out, "<fieldset><legend>引用</legend>正文</fieldset>");
}

#[test]
fn html_descr_is_converted_to_bbcode() {
    // 藏宝阁只吃 BBCode：HTML 原样提交会被转义成字面 <fieldset>，
    // 审核端看不到图/排版（Quick Change 43994 实测）。
    let html = r#"<fieldset><legend> 引用 </legend><br />转自<b>TLF</b>，感谢原发布者！</fieldset><br /><br />
<b><font size="5">本种来自TLFBits<br /><br /></font></b><br />
<img style="max-width: 100%" id="" alt="image" src="https://img9.doubanio.com/view/photo/l_ratio_poster/public/p2300931299.jpg" /><br />
◎IMDb链接　<a class="faqlink" href="https://www.imdb.com/title/tt0100449/" target="_blank">https://www.imdb.com/title/tt0100449/</a><br />"#;
    let bb = html_to_bbcode(html);
    assert!(bb.starts_with("[quote]\n转自[b]TLF[/b]"), "{bb}");
    assert!(bb.contains("[quote]"), "{bb}");
    assert!(bb.contains("[/quote]"), "{bb}");
    assert!(bb.contains("[b][size=5]本种来自TLFBits"), "{bb}");
    assert!(
        bb.contains(
            "[img]https://img9.doubanio.com/view/photo/l_ratio_poster/public/p2300931299.jpg[/img]"
        ),
        "{bb}"
    );
    assert!(
        bb.contains("[url=https://www.imdb.com/title/tt0100449/]"),
        "{bb}"
    );
    assert!(!bb.contains("<fieldset"), "{bb}");
    assert!(!bb.contains("<img"), "{bb}");
    assert!(!bb.contains("&lt;"), "{bb}");
}

#[test]
fn html_to_bbcode_keeps_plain_text_and_repaired_img_url() {
    let text = "用 2 < 3 说明，不动";
    assert_eq!(html_to_bbcode(text), text);
    let html = r#"<img src=" https://x/a.jpg%22" />"#;
    assert_eq!(html_to_bbcode(html), "[img]https://x/a.jpg[/img]");
}

#[test]
fn bbcode_rule_rewrites_descr_field() {
    let mut prep = PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields: vec![(
            "descr".into(),
            "<fieldset><legend>引用</legend>正文</fieldset><img src=\"https://x/a.jpg\">".into(),
        )],
        picked_tags: vec![],
        tag_warn: None,
        select_notes: vec![],
        required_gaps: vec![],
        form_html: String::new(),
        poster_field: None,
        screenshot_field: None,
    };
    let rules = crate::gates::SiteRules {
        descr_format: Some("bbcode".into()),
        ..Default::default()
    };
    apply_site_rules_tail(&mut prep, &rules);
    let d = field_lookup(&prep.fields, "descr").unwrap_or("");
    assert!(d.starts_with("[quote]\n正文"), "{d}");
    assert!(d.contains("[img]https://x/a.jpg[/img]"), "{d}");
    assert!(!d.contains("<fieldset"), "{d}");
    assert!(
        prep.select_notes
            .iter()
            .any(|n| n.contains("HTML 已转 BBCode")),
        "{:?}",
        prep.select_notes
    );
}

#[test]
fn explicit_type_pick_overrides_ambiguous_anime_category() {
    // iloli 只收动画：自动推断落到「动漫/CG杂图/动漫杂志」(411) 时，
    // 站点规则要能改成 405；这只纠正含糊的动漫分支。
    let mut prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![("type".into(), "411".into())],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: r#"<form action="takeupload.php"><select name="type"><option value="411">动漫/CG杂图/动漫杂志</option><option value="405">动画 (Animation)</option></select></form>"#.into(),
            poster_field: None,
            screenshot_field: None,
        };
    let mut rules = crate::gates::SiteRules::default();
    rules.select_picks.insert("type".into(), "405".into());
    apply_site_rules_tail(&mut prep, &rules);
    assert_eq!(field_lookup(&prep.fields, "type"), Some("405"));
}

#[test]
fn explicit_type_pick_does_not_turn_movie_into_anime() {
    // 电影/剧集不能被站点的 type=405 覆盖硬改成动画，否则等于绕过站点只收二次元的规则。
    let mut prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![("type".into(), "401".into())],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: r#"<form action="takeupload.php"><select name="type"><option value="401">电影 (Movie)</option><option value="405">动画 (Animation)</option></select></form>"#.into(),
            poster_field: None,
            screenshot_field: None,
        };
    let mut rules = crate::gates::SiteRules::default();
    rules.select_picks.insert("type".into(), "405".into());
    rules.allowed_types = vec!["动画".into()];
    apply_site_rules_tail(&mut prep, &rules);
    assert_eq!(field_lookup(&prep.fields, "type"), Some("401"));
    assert!(
        prep.required_gaps
            .iter()
            .any(|n| n.contains("不在站点白名单")),
        "{:?}",
        prep.required_gaps
    );
    assert!(
        prep.select_notes
            .iter()
            .any(|n| n.contains("拒绝套用站点 type 覆盖")),
        "{:?}",
        prep.select_notes
    );
}

#[test]
fn default_rules_convert_html_descr_to_bbcode() {
    let html = "<fieldset><legend>引用</legend>正文</fieldset><img src=\"https://x/a.jpg\">";
    let mut prep = PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields: vec![("descr".into(), html.into())],
        picked_tags: vec![],
        tag_warn: None,
        select_notes: vec![],
        required_gaps: vec![],
        form_html: String::new(),
        poster_field: None,
        screenshot_field: None,
    };
    apply_site_rules_tail(&mut prep, &crate::gates::SiteRules::default());
    let d = field_lookup(&prep.fields, "descr").unwrap_or("");
    assert!(d.starts_with("[quote]\n正文"), "{d}");
    assert!(d.contains("[img]https://x/a.jpg[/img]"), "{d}");
    assert!(!d.contains("<fieldset"), "{d}");
}

#[test]
fn html_rule_keeps_descr_html() {
    let html = "<fieldset><legend>引用</legend>正文</fieldset>";
    let mut prep = PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields: vec![("descr".into(), html.into())],
        picked_tags: vec![],
        tag_warn: None,
        select_notes: vec![],
        required_gaps: vec![],
        form_html: String::new(),
        poster_field: None,
        screenshot_field: None,
    };
    let rules = crate::gates::SiteRules {
        descr_format: Some("html".into()),
        ..Default::default()
    };
    apply_site_rules(&mut prep, &rules, None);
    assert_eq!(field_lookup(&prep.fields, "descr"), Some(html));
    assert!(prep.select_notes.is_empty(), "{:?}", prep.select_notes);
}

#[test]
fn single_quoted_ids_are_parsed() {
    let page = r#"<html><body>
<h1>T</h1>
<div id='kdescr'>简介内容 <a href="x">链接</a></div>
<input type='text' name='small_descr' value='副标题'>
</body></html>"#;
    let info = parse_release_page(page, "fallback");
    assert_eq!(info.name, "T");
    assert_eq!(info.subtitle, "副标题");
    assert!(info.descr.contains("简介内容"), "{:?}", info.descr);
    assert!(!info.descr.contains("<html"), "简介兜底不能带标签源码");
}

#[test]
fn descr_fallback_is_body_text_not_raw_html() {
    // 无已知简介容器、也没有「简介」表行：兜底给正文纯文本，不给整页 HTML
    let page = r#"<html><head><style>a{}</style></head><body>
<div class="nav">菜单</div>
<div class="post">真正的简介文字</div>
</body></html>"#;
    let info = parse_release_page(page, "fb");
    assert!(info.descr.contains("真正的简介文字"), "{:?}", info.descr);
    assert!(!info.descr.contains("<div"), "{:?}", info.descr);
}

#[test]
fn truncate_chars_keeps_char_boundary() {
    assert_eq!(truncate_chars("短", 10), "短");
    let t = truncate_chars("中文测试字符串", 3);
    assert!(t.starts_with("中文测"), "{t}");
    assert!(t.contains("…（共 "));
}

#[test]
fn review_render_lists_fields() {
    let rep = ReviewReport {
        info: ReleaseInfo::default(),
        preflight: Preflight::default(),
        prepared: PreparedUpload {
            action: "https://x.example/takeupload.php".into(),
            form_url: "https://x.example/upload.php".into(),
            fields: vec![
                ("name".into(), "T".into()),
                ("descr".into(), "很长的简介".repeat(40)),
            ],
            picked_tags: vec!["cnsub".into()],
            tag_warn: None,
            select_notes: vec!["分类：留空等人工选".into()],
            required_gaps: vec![],
            form_html: String::new(),
            poster_field: None,
            screenshot_field: None,
        },
        audit: Some(AuditVerdict {
            passed: false,
            status: "预审核未通过".into(),
            total_score: 81,
            error_count: 2,
            warning_count: 1,
            suspicious_count: 0,
            details: vec![AuditDetail {
                level: "error".into(),
                message: "MediaInfo 为空".into(),
                score: 30,
                suggestion: Some("补上技术信息".into()),
            }],
            suggestions: vec!["发布前自查".into()],
        }),
        audit_note: None,
        carry: None,
    };
    let t = rep.render(10);
    assert!(t.contains("只审不发"));
    assert!(t.contains("takeupload.php"));
    assert!(t.contains("官方预审（LuckAudit）：❌"));
    assert!(t.contains("总分 81/100"));
    assert!(t.contains("MediaInfo 为空（-30 分）"));
    assert!(t.contains("name = T"));
    assert!(t.contains("descr = "));
    assert!(t.contains("…（共 "), "{t}");
    assert!(t.contains("标签勾选：cnsub"));
}

#[test]
fn verify_posted_full_match_no_warn() {
    let info = ReleaseInfo {
        name: "Show.S01".into(),
        mediainfo: "Unique ID : 1\nFormat : AVC\nBit rate : 3500 Kbps\nWidth : 1920\nHeight : 1080"
            .into(),
        imdb: Some("tt4823720".into()),
        douban: Some("26302614".into()),
        ..Default::default()
    };
    let good = r#"<h1>Show.S01</h1><div id="kdescr">tt4823720 26302614
<pre>Unique ID : 1
Format : AVC
Bit rate : 3500 Kbps
Width : 1920
Height : 1080</pre></div>"#;
    assert!(verify_posted_html(good, &info).is_empty());
}

#[test]
fn verify_posted_detects_missing_parts() {
    let info = ReleaseInfo {
        name: "Show.S01".into(),
        mediainfo: "Unique ID : 1\nFormat : AVC\nBit rate : 3500 Kbps\nWidth : 1920\nHeight : 1080"
            .into(),
        imdb: Some("tt4823720".into()),
        douban: Some("26302614".into()),
        ..Default::default()
    };
    let bad = r#"<h1>别的名字</h1><div id="kdescr">什么都没有</div>"#;
    let w = verify_posted_html(bad, &info);
    assert!(w.iter().any(|x| x.contains("标题与预期不一致")), "{w:?}");
    assert!(w.iter().any(|x| x.contains("MediaInfo")), "{w:?}");
    assert!(w.iter().any(|x| x.contains("tt4823720")), "{w:?}");
    assert!(w.iter().any(|x| x.contains("26302614")), "{w:?}");
}

#[test]
fn parses_official_audit_response() {
    // 真实接口的 level 是大写（ERROR/WARNING/SUSPICIOUS），detail 带 ruleType/errorCode
    let body = r#"{"success":true,"data":{"passed":false,"status":"未通过","totalScore":81,
            "errorCount":2,"warningCount":1,"suspiciousCount":0,
            "details":[{"ruleType":"TECHNICAL","errorCode":"MEDIAINFO_EMPTY","level":"ERROR","message":"MediaInfo 为空","score":30,"suggestion":"补技术信息"},
                       {"ruleType":"TAG","errorCode":"CN_SUB_NOT_TAGGED","level":"WARNING","message":"中字标签未选","score":5}],
            "suggestions":["发布前自查"]}}"#;
    let v = parse_audit_response(200, body).unwrap();
    assert!(!v.passed);
    assert_eq!(v.total_score, 81);
    assert_eq!(v.error_count, 2);
    assert_eq!(v.details.len(), 2);
    assert_eq!(v.details[0].score, 30);
    assert!(v.details[0].line().contains("❌"), "{:?}", v.details[0]);
    assert!(v.details[0].line().contains("MediaInfo 为空（-30 分）"));
    assert!(v.details[1].line().contains("⚠️"));
    assert!(v.summary().contains("总分 81/100"));
    // success=false 走错误路径
    let err = parse_audit_response(200, r#"{"success":false,"message":"登录状态已失效"}"#);
    assert!(err.is_err());
    // 非 JSON（登录墙 HTML）
    assert!(parse_audit_response(200, "<html>login</html>").is_err());
}

#[test]
fn json_payload_flattens_to_php_form() {
    let payload = serde_json::json!({
        "name": "T",
        "type": {"id": "404", "name": "剧集"},
        "quality": {"medium": {"id": "7", "name": "HDTV"}},
        "tags": [{"id": "23", "name": "中字"}],
        "external_links": {"imdb": "https://www.imdb.com/title/tt1/"},
        "note": serde_json::Value::Null,
        "empty_obj": {},
        "empty_arr": [],
    });
    let form = json_to_form(&payload);
    assert!(form.contains(&("name".into(), "T".into())));
    assert!(form.contains(&("type[id]".into(), "404".into())));
    assert!(form.contains(&("type[name]".into(), "剧集".into())));
    assert!(form.contains(&("quality[medium][id]".into(), "7".into())));
    assert!(form.contains(&("tags[0][id]".into(), "23".into())));
    assert!(form.contains(&("tags[0][name]".into(), "中字".into())));
    assert!(form.contains(&(
        "external_links[imdb]".into(),
        "https://www.imdb.com/title/tt1/".into()
    )));
    // null / 空容器不出字段
    assert!(!form.iter().any(|(k, _)| k.starts_with("note")));
    assert!(!form.iter().any(|(k, _)| k.starts_with("empty")));
}

#[test]
fn audit_payload_bits_parse_from_form() {
    // 字段查找：精确 + 下标形式
    let fields: Vec<Field> = vec![
        ("medium_sel[4]".into(), "7".into()),
        ("type".into(), "404".into()),
    ];
    assert_eq!(field_lookup(&fields, "medium_sel"), Some("7"));
    assert_eq!(field_lookup(&fields, "type"), Some("404"));
    assert_eq!(field_lookup(&fields, "codec_sel"), None);

    // select 选项文案 / data-mode
    let page = r#"<select name='type' data-mode='4'>
<option value="0">请选择</option>
<option value="404" data-class-name="cat404">剧集</option></select>
<input type="checkbox" name="tags[4][]" value="23"> 中字
<select name="medium_sel[4]" data-mode="medium_4">
<option value="0">请选择</option><option value="7">HDTV</option></select>"#;
    assert!(pre_audit_supported(&format!(
        "{page}<script src='js/luckaudit.js'></script>"
    )));
    assert!(!pre_audit_supported(page));
    assert_eq!(
        select_option_meta(page, "type", "404"),
        Some(("剧集".into(), "cat404".into()))
    );
    assert_eq!(
        select_option_meta(page, "medium_sel[4]", "7"),
        Some(("HDTV".into(), String::new()))
    );
    assert_eq!(select_mode(page, "type").as_deref(), Some("4"));
    assert_eq!(checkbox_label(page, "23"), "中字");

    // LUCK_AUDIT_USER 抽取
    let with_user = format!(
        "{page}<script>window.LUCK_AUDIT_USER = {{\"id\":42,\"username\":\"u\"}};</script>"
    );
    let u = extract_luck_audit_user(&with_user).expect("user json");
    assert_eq!(u["id"], 42);
    assert!(extract_luck_audit_user(page).is_none());
}

#[test]
fn iso_utc_formats_known_instants() {
    assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000Z");
    // 951782400 = 2000-02-29（闰年）T00:00:00Z
    assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00.000Z");
    assert_eq!(iso_utc(1), "1970-01-01T00:00:01.000Z");
}

#[test]
fn strips_inline_event_handlers_only_inside_tags() {
    // 双引号、单引号、裸值；src/alt 与中文正文原样
    let s = r#"<img alt="图" src="http://x/1.jpg" onload="Scale(this,700,0);" onclick='Preview(this);' />
正文 on=1 online=x 原样 <b>加粗</b>"#;
    assert_eq!(
        strip_event_handlers(s),
        r#"<img alt="图" src="http://x/1.jpg" />
正文 on=1 online=x 原样 <b>加粗</b>"#
    );
    // 没有事件属性时原样返回
    assert_eq!(strip_event_handlers("纯文本<br />结束"), "纯文本<br />结束");
}

#[test]
fn select_keywords_are_longest_first_and_deduped() {
    // WEB-DL 内部的 WEB 被重叠规则吃掉；独立出现的 WEB 仍是词元
    assert_eq!(
        find_keywords("Show.WEB-DL.1080p", MEDIUM_KEYWORDS),
        vec!["WEB-DL"]
    );
    assert_eq!(
        find_keywords("Show.WEB-DL.WEB.WEB-DL.1080p", MEDIUM_KEYWORDS),
        vec!["WEB-DL", "WEB"]
    );
    // 同一关键词重复出现只算一个
    assert_eq!(
        find_keywords("Show.HDTV.HDTV.1080p.HDTV", MEDIUM_KEYWORDS),
        vec!["HDTV"]
    );
    // 词边界：HDD 里的 DD 不算
    assert!(find_keywords("My.HDD.x264", AUDIO_KEYWORDS).is_empty());
    assert_eq!(
        find_keywords("Show.1080p.HDTV.x264.AAC-GRP", STANDARD_KEYWORDS),
        vec!["1080p"]
    );
}

#[test]
fn match_option_prefers_exact_norm_and_rejects_ambiguity() {
    let opts = vec![
        ("7".into(), "HDTV".into()),
        ("8".into(), "WEB-DL 高清".into()),
        ("9".into(), "1080p/1080i".into()),
        ("10".into(), "720p".into()),
        ("11".into(), "1080p".into()),
    ];
    // 中文尾缀不影响全等
    assert_eq!(match_option(&opts, "WEB-DL").as_deref(), Some("8"));
    // 分隔子串（唯一）
    assert_eq!(match_option(&opts, "1080i").as_deref(), Some("9"));
    // 1080p 全等命中两个（1080p/1080i 规范化不同，只有 9？不——norm(9)=1080p1080i）
    assert_eq!(match_option(&opts, "1080p").as_deref(), Some("11"));
    // 词元不存在
    assert_eq!(match_option(&opts, "x264"), None);
}

#[test]
fn remux_medium_prefers_exact_remux_then_bluray() {
    // hdfans/kufei：BD Remux；hdvideo/ptzone/city13：Remux；ptsbao 没有 Remux 档
    let hdfans = vec![
        ("21".into(), "BD原盘".into()),
        ("22".into(), "BD DIY".into()),
        ("23".into(), "BD Remux".into()),
        ("19".into(), "UHD Remux".into()),
        ("1".into(), "H.264/AVC".into()),
    ];
    let hdvideo = vec![
        ("10".into(), "UHD Blu-ray".into()),
        ("12".into(), "Remux".into()),
        ("13".into(), "Encode".into()),
    ];
    let kufei = vec![
        ("4".into(), "BD 原盘".into()),
        ("5".into(), "BD DIY".into()),
        ("6".into(), "BD Remux".into()),
        ("3".into(), "UHD Remux".into()),
    ];
    let ptzone = vec![
        ("1".into(), "Blu-ray".into()),
        ("3".into(), "Remux".into()),
        ("10".into(), "UHD".into()),
    ];
    let ptsbao = vec![
        ("1".into(), "BluRay".into()),
        ("2".into(), "WEB-DL".into()),
        ("5".into(), "HDTV".into()),
    ];
    let city13 = vec![
        ("3".into(), "Remux".into()),
        ("1".into(), "Blu-ray".into()),
        ("11".into(), "BluRay".into()),
    ];
    let hd = "A.Touch.of.Zen.1971.Blu-ray.REMUX.1080p.AVC.LPCM.1.0-WScode";
    let uhd = "Movie.2024.UHD.BluRay.2160p.REMUX.HEVC.DTS-HD.MA.5.1-GRP";
    assert_eq!(pick_remux_option(&hdfans, hd).as_deref(), Some("23"));
    assert_eq!(pick_remux_option(&hdfans, uhd).as_deref(), Some("19"));
    assert_eq!(pick_remux_option(&kufei, hd).as_deref(), Some("6"));
    assert_eq!(pick_remux_option(&kufei, uhd).as_deref(), Some("3"));
    assert_eq!(pick_remux_option(&hdvideo, hd).as_deref(), Some("12"));
    assert_eq!(pick_remux_option(&ptzone, hd).as_deref(), Some("3"));
    assert_eq!(pick_remux_option(&city13, hd).as_deref(), Some("3"));
    assert_eq!(pick_remux_option(&ptsbao, hd).as_deref(), Some("1"));
}

#[test]
fn remux_select_is_filled_for_hdfans_style_form() {
    let form = r#"<form action="takeupload.php">
<select name="medium_sel[4]"><option value="0">请选择一项</option>
<option value="21">BD原盘</option><option value="22">BD DIY</option>
<option value="23">BD Remux</option><option value="19">UHD Remux</option></select>
<select name="codec_sel[4]"><option value="0">请选择一项</option><option value="1">H.264/AVC</option></select>
<select name="audiocodec_sel[4]"><option value="0">请选择一项</option><option value="7">LPCM</option></select>
<select name="standard_sel[4]"><option value="0">请选择一项</option><option value="3">1080P</option></select>
</form>"#;
    let info = ReleaseInfo {
        name: "A.Touch.of.Zen.1971.Blu-ray.REMUX.1080p.AVC.LPCM.1.0-WScode".into(),
        ..Default::default()
    };
    let (fs, _picked, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("23"), "{notes:?}");
    assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("7"));
}

#[test]
fn fill_selects_maps_quality_and_type() {
    let form = r#"<form action="takeupload.php">
<input type="hidden" name="passkey" value="k">
<select name="type"><option value="0">请选择</option><option value="401">电影</option><option value="404">剧集</option><option value="405">综艺</option></select>
<select name="medium_sel[4]"><option value="0">请选择</option><option value="7">HDTV</option><option value="8">WEB-DL</option><option value="9">BluRay</option></select>
<select name="codec_sel[4]"><option value="0">请选择</option><option value="1">x264</option><option value="2">x265</option></select>
<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="1">AAC</option><option value="2">FLAC</option></select>
<select name="standard_sel[4]"><option value="0">请选择</option><option value="1">720p</option><option value="2">1080p</option></select>
<select name="team_sel[4]"><option value="0">请选择</option><option value="5">JING</option><option value="6">WiKi</option></select>
</form>"#;
    let (fs, _picked, notes) = build_upload_fields(&drill_release(), form, &["中字"]);
    let f = |k: &str| field_lookup(&fs, k).unwrap_or("").to_string();
    assert_eq!(f("type"), "404", "国产剧/全41集 → 剧集");
    assert_eq!(f("medium_sel"), "7", "HDTV");
    assert_eq!(f("codec_sel"), "1", "x264");
    assert_eq!(f("audiocodec_sel"), "1", "AAC");
    assert_eq!(f("standard_sel"), "2", "1080p");
    assert_eq!(f("team_sel"), "5", "名尾 -JING");
    assert!(notes.is_empty(), "全部配对成功不该有提示：{notes:?}");
}

#[test]
fn fill_secondary_category_for_ptsbao_style() {
    // ptsbao 实测：source_sel 不是片源，是 type 的二级分类（剧集-大陆/美剧/电影-4K）。
    let form = r#"<form action="takeupload.php">
<select name="type"><option value="0">请选择</option><option value="401">电影</option><option value="402">剧集</option></select>
<select name="source_sel[1]"><option value="0">请选择</option><option value="23">剧集-大陆</option><option value="25">剧集-美剧</option><option value="4">电影-WEB-DL</option><option value="92">电影-4K</option><option value="3">电影-1080</option></select>
</form>"#;
    let tv = ReleaseInfo {
        name: "The.Story.Of.Ming.Lan.2018.WEB-DL.4k.H265.60fps.10bit.AAC-HDSWEB".into(),
        subtitle: "知否知否应是绿肥红瘦 全73集|主演：赵丽颖 冯绍峰".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&tv, form, &[]);
    assert_eq!(field_lookup(&fs, "type"), Some("402"), "{notes:?}");
    assert_eq!(
        field_lookup(&fs, "source_sel"),
        Some("23"),
        "国产剧全N集 → 剧集-大陆：{notes:?}"
    );

    // 电影按分辨率归子类，不是按片源
    let movie = ReleaseInfo {
        name: "Movie.2024.2160p.BluRay.x265-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&movie, form, &[]);
    assert_eq!(field_lookup(&fs, "type"), Some("401"), "{notes:?}");
    assert_eq!(
        field_lookup(&fs, "source_sel"),
        Some("92"),
        "2160p → 电影-4K：{notes:?}"
    );
}

#[test]
fn secondary_category_detection_ignores_medium_hyphens() {
    let medium = vec![
        ("7".into(), "Blu-ray".into()),
        ("8".into(), "UHD Blu-ray".into()),
        ("9".into(), "WEB-DL".into()),
    ];
    assert!(!looks_like_secondary_category(&medium));
    let category = vec![
        ("23".into(), "剧集-大陆".into()),
        ("25".into(), "剧集-美剧".into()),
        ("92".into(), "电影-4K".into()),
    ];
    assert!(looks_like_secondary_category(&category));
}

#[test]
fn secondary_category_skips_region_source_sel() {
    // 别的站 source_sel 是地区（内地/香港），不能按「剧集-」前缀硬套。
    let form = r#"<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<select name="source_sel[4]"><option value="0">请选择</option><option value="1">内地</option><option value="2">香港</option></select>"#;
    let info = ReleaseInfo {
        name: "国产剧.2024.1080p.WEB-DL.x264-GRP".into(),
        subtitle: "全40集".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "source_sel"), Some("1"), "{notes:?}");
    assert!(
        !notes.iter().any(|n| n.contains("二级分类")),
        "地区下拉不该报二级分类：{notes:?}"
    );
}

#[test]
fn fill_selects_notes_when_token_unmatched() {
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="9">BluRay</option></select>"#;
    let info = ReleaseInfo {
        name: "Show.S01.HDTV.1080p".into(),
        ..Default::default()
    };
    let (_fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("medium_sel") && n.contains("HDTV")),
        "HDTV 配不上 BluRay 选项要点名：{notes:?}"
    );
    // 没有质量词元的名字不该有提示
    let (_fs, _p, notes2) = build_upload_fields(
        &ReleaseInfo {
            name: "Plain".into(),
            ..Default::default()
        },
        form,
        &[],
    );
    assert!(notes2.is_empty(), "{notes2:?}");
}

#[test]
fn medium_plain_bluray_without_uhd_picks_plain_label() {
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="10">UHD Blu-ray</option></select>"#;
    let info = ReleaseInfo {
        name: "The.Machinist.2004.BluRay.1080p.TrueHD.5.1.x265-BeiTai".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("1"), "{notes:?}");
}

#[test]
fn bdrip_x264_uses_mediainfo_for_encode_and_resolution() {
    // 藏宝阁实测拒绝：标题只有 BDRip X264，媒介/分辨率都没填。
    let form = r#"<form action="takeupload.php">
<select name="medium_sel[4]"><option value="0">请选择</option><option value="9">UHD</option><option value="8">Remux</option><option value="7">Blu-ray</option><option value="6">WEB-DL</option><option value="5">UHDTV</option><option value="4">HDTV</option><option value="3">Encode</option><option value="2">DVD</option></select>
<select name="codec_sel[4]"><option value="0">请选择</option><option value="9">AV1</option><option value="8">HEVC</option><option value="6">AVC</option></select>
<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="5">DDP</option><option value="4">AAC</option><option value="3">DTS</option><option value="2">AC3</option></select>
<select name="standard_sel[4]"><option value="0">请选择</option><option value="7">4320p</option><option value="6">2160p</option><option value="5">1440p</option><option value="4">1080p</option><option value="3">720p</option><option value="2">540p</option><option value="1">480P</option></select>
</form>"#;
    let info = ReleaseInfo {
            name: "Quick Change 1990 BDRip X264 iNT-TLF".into(),
            mediainfo: "Video\nWidth                                    : 1 290 pixels\nHeight                                   : 696 pixels\nAudio\nFormat                                   : AC-3\n".into(),
            ..Default::default()
        };
    let (fs, _picked, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("3"), "{notes:?}");
    assert_eq!(field_lookup(&fs, "standard_sel"), Some("3"), "{notes:?}");
    assert_eq!(field_lookup(&fs, "codec_sel"), Some("6"), "{notes:?}");
    assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("2"), "{notes:?}");
}

#[test]
fn resolution_from_mediainfo_handles_cropped_frames() {
    assert_eq!(
        resolution_from_mediainfo("Width : 1 920 pixels\nHeight : 800 pixels"),
        Some("1080p")
    );
    assert_eq!(
        resolution_from_mediainfo("Width : 3 840 pixels\nHeight : 1 600 pixels"),
        Some("2160p")
    );
    assert_eq!(
        resolution_from_mediainfo("Width : 1 280 pixels\nHeight : 720 pixels"),
        Some("720p")
    );
}

#[test]
fn medium_compound_token_fills_uhd_bluray_option() {
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="10">UHD Blu-ray</option><option value="3">Remux</option></select>"#;
    let info = ReleaseInfo {
            name: "Star.Wars.Episode.IX.The.Rise.of.Skywalker.2019.UHD.BluRay.1080p.x265.10bit.DDP.7.1.MNHDR-FRDS.mkv".into(),
            ..Default::default()
        };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("10"), "{notes:?}");
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn audio_aliases_match_real_site_labels() {
    // luckpt 实测选项文案
    let opts: Vec<(String, String)> = vec![
        ("11".into(), "TrueHD Atmos".into()),
        ("16".into(), "DTS-HD MA".into()),
        ("14".into(), "TrueHD".into()),
        ("12".into(), "DDP/E-AC3".into()),
        ("8".into(), "DD/AC3".into()),
        ("6".into(), "AAC".into()),
        ("3".into(), "DTS".into()),
    ];
    // DDP → DD+/E-AC3 一类（EC3/EAC3 别名走 fill 路径，见 audio_alias_fill_from_title）
    assert_eq!(match_option(&opts, "DDP").as_deref(), Some("12"));
    // DTS-HD MA → DTS-HD MA
    assert_eq!(match_option(&opts, "DTS-HD MA").as_deref(), Some("16"));
    // TrueHD → TrueHD（不带 Atmos 不抢 Atmos 标签）
    assert_eq!(match_option(&opts, "TrueHD").as_deref(), Some("14"));
}

#[test]
fn audio_alias_fill_from_title() {
    let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="16">DTS-HD MA</option><option value="12">DDP/E-AC3</option><option value="14">TrueHD</option></select>"#;
    let info = ReleaseInfo {
        name: "Dark.City.1998.DC.BluRay.1080p.2Audio.DTS-HD.MA.7.1.x265.10bit-BeiTai".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("16"), "{notes:?}");
    let info2 = ReleaseInfo {
        name: "Show.2020.1080p.BluRay.EC3.5.1-x265-GRP".into(),
        ..Default::default()
    };
    let (fs2, _p2, notes2) = build_upload_fields(&info2, form, &[]);
    assert_eq!(
        field_lookup(&fs2, "audiocodec_sel"),
        Some("12"),
        "{notes2:?}"
    );
}

#[test]
fn resolution_case_and_i_variants_normalize() {
    let opts: Vec<(String, String)> = vec![
        ("8".into(), "1080p".into()),
        ("9".into(), "1080i".into()),
        ("10".into(), "720p".into()),
    ];
    // 大小写归一：1080P / 1080p 同选项
    assert_eq!(match_option(&opts, "1080P").as_deref(), Some("8"));
    assert_eq!(match_option(&opts, "1080p").as_deref(), Some("8"));
    assert_eq!(match_option(&opts, "1080I").as_deref(), Some("9"));
    assert_eq!(match_option(&opts, "1080i").as_deref(), Some("9"));
}

#[test]
fn episode_marker_picks_tv_not_movie() {
    let form = r#"<select name="type"><option value="0">请选择</option><option value="401">电影</option><option value="402">电视剧</option><option value="405">动漫</option></select>"#;
    // 带 E01-E21 集数特征 → 唯一定位电视剧，不判歧义
    let info = ReleaseInfo {
        name: "白色巨塔.E01-E21.2003.1080p.HDTV.x265.AC3-FRDS".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "type"), Some("402"), "{notes:?}");
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn movie_with_year_no_episode_stays_movie() {
    let form = r#"<select name="type"><option value="0">请选择</option><option value="401">电影</option><option value="402">电视剧</option></select>"#;
    // 无集数特征的年份电影 → 电影，不误伤成剧集
    let info = ReleaseInfo {
        name: "Dark.City.1998.DC.BluRay.1080p.DTS-HD.MA.7.1.x265-BeiTai".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "type"), Some("401"), "{notes:?}");
}

#[test]
fn upload_descr_strips_inline_event_handlers() {
    // WAF 实测：真实上传也会被 on* 属性拦（luckpt 403）；提交前统一剥掉
    let form = r#"<form action="takeupload.php"><input type="file" name="file"><textarea name="descr"></textarea></form>"#;
    let info = ReleaseInfo {
        name: "T".into(),
        descr: r#"<img src="http://x/y.jpg" onload="Scale(this)" onclick="Preview(this)">正文"#
            .into(),
        ..Default::default()
    };
    let (fs, _p, _n) = build_upload_fields(&info, form, &[]);
    let d = field_lookup(&fs, "descr").unwrap_or("");
    assert!(!d.contains("onload") && !d.contains("onclick"), "{d}");
    assert!(d.contains("正文") && d.contains("y.jpg"), "{d}");
}

#[test]
fn post_title_normalization() {
    assert_eq!(
        norm_post_title("Dark City 1998 DC BluRay 1080p-BeiTai"),
        norm_post_title("dark.city.1998.dc.bluray.1080p.beitai")
    );
    assert_ne!(
        norm_post_title("Dark City 1998"),
        norm_post_title("Dark City 1999")
    );
}

#[test]
fn site_title_badges_are_stripped_before_matching() {
    let want = "The Disguiser Complete HDTV 1080p AAC x264-JING";
    let got = "The Disguiser Complete HDTV 1080p AAC x264-JING [免费] 剩余时间：23时59分钟";
    assert_eq!(strip_site_title_badges(got), want);
    assert_eq!(
        norm_post_title(strip_site_title_badges(got)),
        norm_post_title(want)
    );
    // 正常不带促销后缀的标题不能误裁
    assert_eq!(strip_site_title_badges(want), want);
    // 多个角标的顺序和内置列表不同时，要裁在标题里最早出现的那个角标上
    assert_eq!(
        strip_site_title_badges("Some Pack 2020 1080p [2X] [Free]"),
        "Some Pack 2020 1080p"
    );
    assert_eq!(
        strip_site_title_badges("Some Pack 2020 1080p [50%] [Promotion] 剩余时间：1时"),
        "Some Pack 2020 1080p"
    );
    // 13city 的合并角标：[2X免费] 不能因为包含 [免费] 被裁成半截
    assert_eq!(
            strip_site_title_badges(
                "Agatha Christies Poirot S01-S13 Collection 1080p Bluray x265 10bit MNHD-FRDS [2X免费] 剩余时间：23时59分钟"
            ),
            "Agatha Christies Poirot S01-S13 Collection 1080p Bluray x265 10bit MNHD-FRDS"
        );
    // 角标在最前面时没有可返回的标题前缀，保持原样
    assert_eq!(
        strip_site_title_badges("[Free] Some Pack 2020 1080p"),
        "[Free] Some Pack 2020 1080p"
    );
}

#[test]
fn mediainfo_nbsp_is_normalized() {
    let s = normalize_mediainfo("Duration\u{a0}\u{a0}: 2 h\r\nWidth\u{a0}: 1920");
    assert!(s.contains("Duration  : 2 h") && !s.contains('\u{a0}') && !s.contains('\r'));
}

#[test]
fn mediainfo_completeness_requires_duration_and_dimensions() {
    assert!(mediainfo_is_complete(
        "General
Duration : 47 min
Video
Width : 1920 pixels
Height : 1080 pixels"
    ));
    assert!(!mediainfo_is_complete(
        "General
Format : MPEG-4
Video
Format : AVC"
    ));
}

#[test]
fn chinese_text_track_counts_as_subs() {
    let info = ReleaseInfo {
        mediainfo: "Text #1
Language : Chinese
Title : 简体中文"
            .into(),
        ..Default::default()
    };
    assert!(info.has_chinese_subs());
    let only_english = ReleaseInfo {
        mediainfo: "Text #1
Language : English"
            .into(),
        ..Default::default()
    };
    assert!(!only_english.has_chinese_subs());
}

#[test]
fn bluray_remux_maps_to_remux_option() {
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="2">Remux</option><option value="3">Encode</option></select>"#;
    let info = ReleaseInfo {
        name: "A Touch of Zen 1971 Blu-ray REMUX 1080p AVC LPCM".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("2"), "{notes:?}");
}

#[test]
fn qingwa_remux_title_and_tag_are_fixed() {
    let form = r#"<input type="text" name="name" value="">
<select name="source_sel[4]"><option value="0">请选择</option><option value="1">UHD Blu-ray</option><option value="8">Blu-ray</option><option value="9">Remux</option><option value="10">Encode</option></select>
<label><input type="checkbox" name="tags[4][]" value="15" />Remux</label>"#;
    let info = ReleaseInfo {
        name: "A Touch of Zen 1971 1080p Blu-ray REMUX AVC LPCM 1.0-WScode".into(),
        ..Default::default()
    };
    let tags = wanted_tags(&info);
    assert!(tags.contains(&"Remux"), "Remux 没进想勾的标签：{tags:?}");
    let (fs, picked, notes) = build_upload_fields_for_site(&info, form, &tags, "qingwa");
    assert_eq!(
        field_lookup(&fs, "name"),
        Some("A Touch of Zen 1971 1080p BluRay REMUX AVC LPCM 1.0-WScode"),
        "{notes:?}"
    );
    assert_eq!(field_lookup(&fs, "source_sel[4]"), Some("9"), "{notes:?}");
    assert!(
        picked.contains(&"15".to_string()),
        "Remux 标签没勾上：{picked:?}"
    );
}

#[test]
fn checkbox_exact_label_beats_prefix_match() {
    let form = r#"<input type="checkbox" name="tags[4][]" value="1" />HDR10<input type="checkbox" name="tags[4][]" value="2" />HDR10+"#;
    let got = match_checkboxes(form, "HDR10");
    assert_eq!(got, vec![("tags[4][]".to_string(), "1".to_string())]);
}

#[test]
fn chinese_prefix_title_is_trimmed_only_when_english_exists() {
    assert_eq!(
        normalize_release_title("[雄狮少年].I.Am.What.I.Am.2021.CHN.BluRay.1080p-GRP"),
        "I.Am.What.I.Am.2021.CHN.BluRay.1080p-GRP"
    );
    assert_eq!(
        normalize_release_title("武状元苏乞儿.King.of.Beggars.1992.BluRay.1080p-GRP"),
        "King.of.Beggars.1992.BluRay.1080p-GRP"
    );
    assert_eq!(
        normalize_release_title("纯中文标题.2018.1080p"),
        "纯中文标题.2018.1080p"
    );
    // 中文名后面直接跟技术词时，不能把中文名裁掉；白色巨塔实测曾生成「p HDTV…」
    assert_eq!(
        normalize_release_title("白色巨塔.E01-E21.2003.1080p.HDTV.x265.AC3￡cXcY@FRDS"),
        "白色巨塔.E01-E21.2003.1080p.HDTV.x265.AC3￡cXcY@FRDS"
    );
}

#[test]
fn release_tech_tokens_are_reordered_in_place() {
    assert_eq!(
        normalize_release_title_for("The.Disguiser.Complete.HDTV.1080p.AAC.x264-JING", true),
        "The.Disguiser.1080p.HDTV.x264.AAC-JING"
    );
    assert_eq!(
        normalize_release_title_for(
            "The Story Of Ming Lan 2018 WEB-DL 4k H265 60fps 10bit AAC-HDSWEB",
            true
        ),
        "The Story Of Ming Lan 2018 4k 60fps WEB-DL H265 10bit AAC-HDSWEB"
    );
    assert_eq!(
        normalize_release_title_for(
            "Agatha Christies Poirot S01-S13 Collection Bluray 1080p x265 10bit MNHD-FRDS",
            true
        ),
        "Agatha Christies Poirot S01-S13 Collection 1080p Bluray x265 10bit MNHD-FRDS"
    );
    // 中文前缀里的合集季范围不能丢：Mad Men 合集曾从 S01-S07 掉成 S07
    assert_eq!(
        normalize_release_title_for(
            "广告狂人S01-S07.Mad.Men.2007-2014.1080p.Blu-ray.x265.AC3￡cXcY@FRDS",
            true
        ),
        "Mad.Men.S01-S07.2007-2014.1080p.Blu-ray.x265.AC3￡cXcY@FRDS"
    );
    assert_eq!(
        season_range("广告狂人S01-S07.Mad.Men"),
        Some("S01-S07".to_string())
    );
    assert_eq!(season_range("Mad.Men.S07"), None);
    // 技术词之后的末段整体保留在最后，不要被挪到标题主体前面
    assert_eq!(
        normalize_release_title_for(
            "S07 Mad Men 2007-2014 1080p Blu-ray x265 AC3￡cXcY@FRDS",
            true
        ),
        "S07 Mad Men 2007-2014 1080p Blu-ray x265 AC3￡cXcY@FRDS"
    );
    // 地区、声道/音频细节跟着技术顺序走，压制组仍在最后
    assert_eq!(
        normalize_release_title_for(
            "Coraline.2009.1080p.USA.Blu-ray.AVC.DTS-HD.MA.5.1-PIXELPOLICE",
            true
        ),
        "Coraline.2009.1080p.Blu-ray.USA.AVC.DTS-HD.MA.5.1-PIXELPOLICE"
    );
    // 技术词之后不认识的字样（2Audio、MNHD-FRDS）整体留在最后
    assert_eq!(
        normalize_release_title_for(
            "Stephen.Chow.1988-2017.BluRay.1080p.x265.10bit.2Audio.MNHD-FRDS",
            true
        ),
        "Stephen.Chow.1988-2017.1080p.BluRay.x265.10bit.2Audio.MNHD-FRDS"
    );
    // 方括号中文前缀先按既有规则去掉，再重排技术参数
    assert_eq!(
            normalize_release_title_for(
                "[人民的名义].In.the.Name.of.People.2017.S01.Complete.2160p.60fps.WEB-DL.HEVC.10bit.AAC-QHstudIo",
                true
            ),
            "In.the.Name.of.People.2017.S01.2160p.60fps.WEB-DL.HEVC.10bit.AAC-QHstudIo"
        );
}

#[test]
fn hdr10_tag_detected_from_uhd_title() {
    let info = ReleaseInfo {
        name: "Movie.2024.2160p.UHD.BluRay.HDR10.x265-GRP".into(),
        ..Default::default()
    };
    assert!(wanted_tags(&info).contains(&"HDR10"));

    let sdr = ReleaseInfo {
        name: "Movie.2018.1080p.BluRay.x265-GRP".into(),
        mediainfo: "Video\nHDR format : HDR10".into(),
        ..Default::default()
    };
    assert!(!wanted_tags(&sdr).contains(&"HDR10"));
}

#[test]
fn audio_codec_falls_back_to_mediainfo() {
    let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="8">DD/AC3</option><option value="12">DDP/E-AC3</option><option value="3">DTS</option></select>"#;
    let info = ReleaseInfo {
        name: "Movie.2014.1080p.BluRay.x265-GRP".into(),
        mediainfo: "Audio #1
Format : AC-3
Language : English"
            .into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("8"), "{notes:?}");
}

#[test]
fn ddplus_maps_to_ddp_eac3() {
    let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="8">DD/AC3</option><option value="12">DDP/E-AC3</option></select>"#;
    let info = ReleaseInfo {
        name: "Movie.2014.1080p.BluRay.x265.DD+5.1-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("12"), "{notes:?}");
}

#[test]
fn ac3_slash_dd_label_maps_to_ac3_option() {
    // iloli 的真实标签顺序是 AC3/DD（不是 DD/AC3）
    let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择一项</option><option value="6">AAC</option><option value="13">AC3/DD</option><option value="20">DDP/E-AC3</option><option value="3">DTS</option></select>"#;
    let info = ReleaseInfo {
        name: "Ghiblies.Episode.2.2002.1080p.BluRay.10bit.DD5.1.x265-MNHD-FRDS".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("13"), "{notes:?}");
}

#[test]
fn h265_without_dot_maps_to_hevc_option() {
    let form = r#"<select name="codec_sel[4]"><option value="0">请选择</option><option value="1">H.264/AVC</option><option value="6">H.265/HEVC</option></select>"#;
    let info = ReleaseInfo {
        name: "Show.2018.WEB-DL.4k.H265.60fps.10bit.AAC-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "codec_sel"), Some("6"), "{notes:?}");
}

#[test]
fn ac3_prefers_dd_ac3_not_eac3() {
    let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="8">DD/AC3</option><option value="12">DDP/E-AC3</option><option value="3">DTS</option></select>"#;
    let info = ReleaseInfo {
        name: "Movie.2023.1080p.BluRay.x264.AC3-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "audiocodec_sel"), Some("8"), "{notes:?}");
}

#[test]
fn cantonese_audio_tag() {
    let info = ReleaseInfo {
        mediainfo: "Audio #1
Title : 粤语
Language : Chinese"
            .into(),
        ..Default::default()
    };
    assert!(info.has_cantonese_audio());
}

#[test]
fn prune_dead_images_keeps_reachable_only() {
    let html = r#"正文<img src="http://ok/1.jpg"><img src="http://dead/2.jpg">尾"#;
    let (out, n) = prune_dead_images_with(html, &mut |u| u.contains("ok"));
    assert_eq!(n, 1);
    assert!(
        out.contains("ok/1.jpg") && !out.contains("dead/2.jpg"),
        "{out}"
    );
}

#[test]
fn prune_known_hotlink_and_imgur_even_when_probe_says_ok() {
    let html = r#"正文<img src="https://hdsky.me/a.jpg"><img src="https://i.imgur.com/b.jpg"><img src="https://img3.pixhost.to/images/1/c.jpg">"#;
    let (out, n) =
        prune_dead_images_with_site(html, "https://cangbao.ge/upload.php", &mut |_| true);
    assert_eq!(n, 2, "{out}");
    assert!(out.contains("img3.pixhost.to"), "{out}");
    assert!(
        !out.contains("hdsky.me") && !out.contains("imgur.com"),
        "{out}"
    );
}

#[test]
fn prune_keeps_injected_pixhost_without_probe() {
    let html = r#"<img src="https://img3.pixhost.to/images/1/a.jpg"><img src="https://unknown.example/b.jpg">"#;
    let (out, n) = prune_dead_images_with(html, &mut |_| false);
    assert_eq!(n, 1);
    assert!(
        out.contains("pixhost") && !out.contains("unknown.example"),
        "{out}"
    );
}

#[test]
fn prune_imgbox_on_ubits_but_not_other_sites() {
    let html = r#"<img src="https://images4.imgbox.com/a.jpg">"#;
    let (out, n) =
        prune_dead_images_with_site(html, "https://ubits.club/upload.php", &mut |_| true);
    assert_eq!(n, 1, "{out}");
    let (out2, n2) =
        prune_dead_images_with_site(html, "https://qingwapt.com/upload.php", &mut |_| true);
    assert_eq!(n2, 0);
    assert!(out2.contains("imgbox.com"));
}

#[test]
fn prune_bbcode_hotlink_keeps_pixhost() {
    let text = "正文[img]https://hdsky.me/a.jpg[/img][img=200x100]https://img3.pixhost.to/images/1/a.jpg[/img]";
    let (out, n) =
        prune_dead_images_with_site(text, "https://cangbao.ge/upload.php", &mut |_| true);
    assert_eq!(n, 1, "{out}");
    assert!(
        out.contains("pixhost") && !out.contains("hdsky.me"),
        "{out}"
    );
}

#[test]
fn image_content_type_rejects_hotlink_pages() {
    assert!(content_type_is_image(Some("image/jpeg")));
    assert!(content_type_is_image(Some("IMAGE/PNG")));
    assert!(content_type_is_image(Some("application/octet-stream")));
    // 防盗链/验证页：HTML、JSON 不算图片
    assert!(!content_type_is_image(Some("text/html; charset=UTF-8")));
    assert!(!content_type_is_image(Some("application/json")));
    // CT 缺失放行（图床不回 CT 时不误剪）
    assert!(content_type_is_image(None));
}

#[test]
fn upload_dump_gate_is_off_by_default_and_env_gated() {
    // 默认关闭：未设置 / 空串 / 0 / 无关值都不落盘
    assert!(!dump_upload_enabled(None));
    assert!(!dump_upload_enabled(Some("")));
    assert!(!dump_upload_enabled(Some("0")));
    assert!(!dump_upload_enabled(Some("no")));
    assert!(!dump_upload_enabled(Some("yes")));
    // 显式开启：大小写/空白不敏感
    assert!(dump_upload_enabled(Some("1")));
    assert!(dump_upload_enabled(Some("true")));
    assert!(dump_upload_enabled(Some(" on ")));
    assert!(dump_upload_enabled(Some("TRUE")));
}

#[test]
fn bluray_avc_is_not_reencode() {
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="2">Encode</option></select>"#;
    let info = ReleaseInfo {
        name: "Bitter Moon 1992 1080p Blu-ray AVC DTS-HD MA 2.0".into(),
        ..Default::default()
    };
    let (fs, _p, _n) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("1"));
}

#[test]
fn qingwa_bluray_requires_bdinfo_and_picks_other() {
    const BDINFO: &str = "Disc Label: Bitter Moon 1992 1080p ESP Blu-ray AVC DTS-HD MA 2.0\n\
Disc Size: 40,338,327,962 bytes\n\
Playlist: 00001.MPLS\n\
Video: MPEG-4 AVC Video / 33841 kbps / 1080p / 23.976 fps\n\
Audio: Spanish / Dolby Digital Audio / 2.0 / 48 kHz / 192 kbps\n\
Subtitle: Spanish / Portuguese / Italian / English";
    let html = format!("<pre>{BDINFO}</pre>");
    assert!(is_bdinfo_block(BDINFO));
    assert_eq!(extract_bdinfo(&html, ""), BDINFO);
    assert!(mediainfo_is_complete(BDINFO));

    let mi = "General\nDuration : 2h 19m\nVideo\nWidth : 1920\nHeight : 1080";
    let mut bad = ReleaseInfo {
        name: "Bitter Moon 1992 1080p Blu-ray AVC DTS-HD MA 2.0".into(),
        descr: "IMDb: https://www.imdb.com/title/tt0104777/".into(),
        mediainfo: mi.into(),
        ..Default::default()
    };
    let pf = preflight(&mut bad, &[], &crate::gates::SiteRules::default(), "qingwa");
    assert!(
        pf.errors.iter().any(|e| e.contains("BDInfo")),
        "{:?}",
        pf.errors
    );

    let form = r#"<form action="takeupload.php">
<input type="text" name="name" value="">
<input type="text" name="small_descr" value="">
<textarea name="descr"></textarea>
<textarea name="technical_info"></textarea>
<select name="audiocodec_sel[4]"><option value="0">请选择</option><option value="10">DTS-HD MA</option><option value="7">Other</option></select>
<select name="source_sel[4]"><option value="0">请选择</option><option value="8">Blu-ray</option></select>
<input type="checkbox" name="tags[4][]" value="10"> 原生原盘
<input type="checkbox" name="tags[4][]" value="6"> 特效字幕
<input type="checkbox" name="tags[4][]" value="8"> 完结
<input type="checkbox" name="tags[4][]" value="14"> 系列合集
</form>"#;
    let info = ReleaseInfo {
        name: "Bitter Moon 1992 1080p Blu-ray AVC DTS-HD MA 2.0".into(),
        mediainfo: BDINFO.into(),
        ..Default::default()
    };
    let tags = wanted_tags(&info);
    assert!(tags.contains(&"原生原盘"), "{tags:?}");
    let (fs, picked, _notes) = build_upload_fields_for_site(&info, form, &tags, "qingwa");
    assert_eq!(field_lookup(&fs, "audiocodec_sel[4]"), Some("7"));
    assert!(picked.contains(&"10".to_string()), "{picked:?}");
}

#[test]
fn chinese_audio_detection() {
    let mk = |mi: &str| ReleaseInfo {
        mediainfo: mi.into(),
        ..Default::default()
    };
    assert!(mk("Audio #1
Language : English
Audio #2
Language : Chinese")
    .has_chinese_audio());
    assert!(mk("Audio #1
Title : Mandarin 2.0 国配")
    .has_chinese_audio());
    assert!(!mk("Audio #1
Language : English
Text #1
Language : Chinese")
    .has_chinese_audio());
    let t = wanted_tags(&mk("Audio #1
Language : Chinese"));
    assert!(t.contains(&"国语"), "{t:?}");
}

#[test]
fn completed_series_detection() {
    let mk = |n: &str, s: &str| ReleaseInfo {
        name: n.into(),
        subtitle: s.into(),
        ..Default::default()
    };
    assert!(mk("The.Disguiser.Complete.HDTV.1080p", "").is_completed_series());
    assert!(mk("Show.S01.1080p", "全41集").is_completed_series());
    assert!(mk("Show.S01.1080p", "已完结").is_completed_series());
    assert!(!mk("Show.S01.1080p", "更新至第10集").is_completed_series());
    // wanted_tags：中字 + 完结都要在
    let t = wanted_tags(&mk("Show.Complete", "中字"));
    assert!(t.contains(&"中字") && t.contains(&"完结"), "{t:?}");
}

#[test]
fn type_inference_variants() {
    let g = infer_type_candidates("The Disguiser Complete", "国产剧--伪装者 全41集").unwrap();
    assert!(g.0.contains(&"国产剧"));
    assert!(infer_type_candidates("Show.S01.1080p", "").is_some());
    assert!(infer_type_candidates("Show.第3季", "").is_some());
    // 年份 + 无集数标记 = 电影惯用命名（强信号；仍由官方预审做最终闸门）
    let m = infer_type_candidates("Dune.2021.1080p.BluRay.x264-GRP", "").unwrap();
    assert!(m.0.contains(&"电影"));
    assert!(infer_type_candidates("Plain.Title.1080p", "").is_none());
}

#[test]
fn type_inference_prefers_genre_line_from_descr() {
    let descr =
        "导演: 百瀬義行<br>类型: 喜剧 / 动画 / 短片 / 奇幻<br>制片国家/地区: 日本<br>语言: 日语";
    let hint = infer_type_from_descr(descr).unwrap();
    assert_eq!(hint.0[0], "动画");
    // 简介正文里没有明确「类型」行时不猜
    assert!(infer_type_from_descr("这是一部关于动画制作人的纪录片").is_none());
}

#[test]
fn release_group_takes_tail_suffix() {
    assert_eq!(
        release_group("The.Disguiser.Complete.HDTV.1080p.AAC.x264-JING").as_deref(),
        Some("JING")
    );
    assert_eq!(release_group("Movie.2020.1080p.BluRay.x264-10bit"), None);
    assert_eq!(release_group("Show.S01"), None);
}

#[test]
fn form_parsing_scoped_to_upload_form() {
    // 导航条搜索表单（search_area）不能掺进上传字段
    let page = r#"<form action="search.php"><input name="search"><select name="search_area"><option value="1">全站</option></select></form>
<form action="takeupload.php" method="post"><input type="hidden" name="passkey" value="k">
<select name="type"><option value="0">请选择</option><option value="404">剧集</option></select></form>"#;
    let fields = parse_form_fields(page);
    assert!(fields.contains(&("passkey".into(), "k".into())));
    assert!(fields.contains(&("type".into(), "0".into())));
    assert!(
        !fields.iter().any(|(n, _)| n == "search_area"),
        "{fields:?}"
    );
    assert_eq!(select_names(page), vec!["type".to_string()]);
    assert!(upload_form_block("<p>无表单</p>").is_none());
}

#[test]
fn form_action_does_not_fall_back_to_donation_form() {
    // longpt 实测：upload.php 里只有 bonusrain.php 捐款表单，没有上传表单。
    // 不能退回整页取第一个 form，否则会把种子 POST 到捐款接口。
    let page = r#"<form action="bonusrain.php" method="post">
<input type="hidden" name="action" value="donate"><input type="number" name="amount"></form>"#;
    assert!(upload_form_block(page).is_none());
    assert_eq!(
        form_action(page, &Url::parse("https://longpt.org/upload.php").unwrap()),
        None
    );
}

#[test]
fn checkbox_grid_does_not_bleed_into_neighbor() {
    // 密集标签网格：第一个复选框的 80 字节窗口会盖住邻格「中字」，不能误配
    let page = r#"<input type="checkbox" name="tags[]" value="10"> 原创首发高清首发字幕首发
<input type="checkbox" name="tags[]" value="6"> 中字"#;
    assert_eq!(
        match_checkboxes(page, "中字"),
        vec![("tags[]".to_string(), "6".to_string())]
    );
}

#[test]
fn type_syns_match_sequentially_not_pooled() {
    // hdfans 式双语选项：短词 TV 同时命中「TV Series/电视剧」与「TV Shows/综艺」，
    // 逐同义词唯一命中才能落到电视剧
    let form = r#"<select name="type"><option value="0">请选择</option>
<option value="401">Movies/电影</option>
<option value="402">TV Series/电视剧</option>
<option value="416">TV Shows/综艺</option></select>"#;
    let info = ReleaseInfo {
        name: "The.Disguiser.Complete.1080p.HDTV.x264.AAC-JING".into(),
        subtitle: "国产剧--伪装者 全41集".into(),
        ..Default::default()
    };
    let (fs, _p, _n) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "type"), Some("402"));
}

#[test]
fn gate_messages_name_the_blocker_not_missing_form() {
    use crate::sites::upload::UploadPageKind;
    let challenge = gate_error_message(
        UploadPageKind::ChallengeGate,
        "mypt",
        "https://pt.mypt.cc/upload.php",
        "",
    )
    .unwrap();
    assert!(challenge.contains("前台") && challenge.contains("人工"));
    // 人机验证页不能被报成「没有可解析的静态上传表单」
    assert!(!challenge.contains("没有可解析的静态上传表单"));

    let spa = gate_error_message(
        UploadPageKind::SpaApi,
        "rousi",
        "https://rousi.pro/upload.php",
        "",
    )
    .unwrap();
    assert!(spa.contains("SPA/JSON API") && spa.contains("缺："));

    let perm = gate_error_message(
        UploadPageKind::NoPermission,
        "ttg",
        "https://totheglory.im/upload.php",
        "您没有上传种子的权限",
    )
    .unwrap();
    assert!(perm.contains("[no_permission]") && perm.contains("上传种子的权限"));

    let twofa = gate_error_message(
        UploadPageKind::TwoFa,
        "hddolby",
        "https://www.uhddolby.com/upload.php",
        "",
    )
    .unwrap();
    assert!(twofa.contains("[two_fa]") && twofa.contains("二次验证"));
    assert!(!twofa.contains("没有可解析的静态上传表单"));

    // 静态表单不中止，返回 None 走原字段构造路径
    assert!(gate_error_message(UploadPageKind::StaticForm, "x", "u", "").is_none());
}

#[test]
fn site_block_kind_maps_four_blockers_only() {
    // 四类站侧阻塞映射到独立审计分类；登录失效 / 认不出表单 / 网络错误仍走旧失败路径。
    assert_eq!(
        site_block_kind("[no_permission] 该账号在 ttg 无发布权限"),
        Some("no_permission")
    );
    assert_eq!(
        site_block_kind("[two_fa] 站点要求先过二次验证"),
        Some("two_fa")
    );
    assert_eq!(
        site_block_kind("[challenge_gate] 站点被人机验证拦截"),
        Some("challenge_gate")
    );
    assert_eq!(
        site_block_kind("[spa_api] 站点上传界面是纯前端 SPA/JSON API"),
        Some("spa_api")
    );
    assert!(site_block_kind("[login] 登录态失效").is_none());
    assert!(site_block_kind("[no-form] 没有可解析的静态上传表单").is_none());
    assert!(site_block_kind("请求没通：operation timed out").is_none());
    assert!(site_block_kind("").is_none());
}

#[test]
fn denial_reason_covers_traditional_chinese() {
    use crate::sites::upload::UploadPageKind;
    let t = "對不起... 你沒有發布種子的許可權。如果你是普通會員，請到 候選區 提交你的發布請求";
    assert_eq!(classify_upload_page(t), UploadPageKind::NoPermission);
    assert_eq!(
        classify_upload_page("对不起... 你没有发布种子的权限"),
        UploadPageKind::NoPermission
    );
    assert_ne!(
        classify_upload_page("<p>随便看看</p>"),
        UploadPageKind::NoPermission
    );
}

#[test]
fn fixed_windows_do_not_split_utf8() {
    // 任意窗口端点都回退到字符边界，切片不 panic
    let s = "中文标签中字测试";
    for i in 0..=s.len() + 4 {
        let f = floor_char_boundary(s, i);
        assert!(s.is_char_boundary(f));
        let _ = &s[..f];
    }
    // 定长 80 字节窗口切在汉字中间的真实页面（实机 panic 过）
    let page = format!(
        r#"<input type="checkbox" name="tags[4][]" value="23"> 中字{}"#,
        "字".repeat(40)
    );
    assert_eq!(match_checkboxes(&page, "中字").len(), 1);
    assert!(checkbox_label(&page, "23").contains("中字"));
}

#[test]
fn analyze_form_reports_core_and_soft_gaps() {
    let prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![
                ("name".into(), "T".into()),
                ("small_descr".into(), "s".into()),
                ("descr".into(), "d".into()),
                ("technical_info".into(), "m".into()),
                ("imdb_url".into(), "u".into()),
                ("type".into(), "0".into()),
                ("medium_sel[4]".into(), "7".into()),
            ],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec!["分类：留空等人工选".into()],
            required_gaps: vec![],
            form_html: r#"<select name="type"><option value="0">请选择</option><option value="404">剧集</option></select>
<select name="medium_sel[4]"><option value="0">请选择</option><option value="7">HDTV</option></select>
<select name="codec_sel[4]"><option value="0">请选择</option><option value="1">x264</option></select>"#
                .into(),
            poster_field: None,
            screenshot_field: None,
        };
    let info = ReleaseInfo {
        name: "T".into(),
        subtitle: "s".into(),
        ..Default::default()
    };
    let d = analyze_form("sx", "站X", &prep, &info);
    assert!(d.title_set && d.subtitle_set);
    assert_eq!(d.mediainfo_field.as_deref(), Some("technical_info"));
    assert_eq!(d.link_fields, vec!["imdb_url".to_string()]);
    assert_eq!(
        d.select_picks,
        vec![("medium_sel[4]".into(), "HDTV".into())]
    );
    let core = d.core_gaps();
    assert!(core.iter().any(|g| g.contains("type")), "{core:?}");
    assert!(core.iter().any(|g| g.contains("codec_sel")), "{core:?}");
    let soft = d.soft_gaps();
    assert!(soft.iter().any(|g| g.contains("分类")), "{soft:?}");
}

#[test]
fn hdtv_x264_maps_to_encode_when_no_hdtv_option() {
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="2">Encode</option><option value="3">Web-DL</option></select>"#;
    let info = ReleaseInfo {
        name: "Show.Complete.HDTV.1080p.x264-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, _notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("2"));
    // 音乐站没有 Encode：不硬塞，保持留空（站点不收该内容）
    let music = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">CD</option><option value="2">SACD</option></select>"#;
    let (fs, _p, _n) = build_upload_fields(&info, music, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("0"));
}

#[test]
fn reencoded_disc_maps_to_encode() {
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="1">Blu-ray</option><option value="2">Encode</option><option value="3">Remux</option></select>"#;
    let info = ReleaseInfo {
        name: "Dark.City.1998.BluRay.1080p.x265.10bit-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("2"), "{notes:?}");
    // REMUX 仍按原盘归类，不改判 Encode
    let remux = ReleaseInfo {
        name: "Dark.City.1998.BluRay.REMUX.1080p-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, _n) = build_upload_fields(&remux, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("3"));
}

#[test]
fn picks_2160p_60fps_option_when_option_text_has_both() {
    let options = vec![
        ("5".to_string(), "2160p/4K".to_string()),
        ("7".to_string(), "2160p/4K 60Fps".to_string()),
    ];
    assert_eq!(
        pick_resolution_option(&options, "In the Name of People 2017 2160p 60fps WEB-DL"),
        Some("7".to_string())
    );
    assert_eq!(
        pick_resolution_option(&options, "In the Name of People 2017 2160p WEB-DL"),
        Some("5".to_string())
    );
}

#[test]
fn webdl_maps_to_encode_when_no_web_option() {
    // oshen 实测：没有 WEB-DL，只有 HDTV/Encode/Blu-ray；留空会被发布检查拦。
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="5">HDTV</option><option value="7">Encode</option><option value="3">Remux</option><option value="1">Blu-ray</option></select>"#;
    let info = ReleaseInfo {
        name: "Show.WEB-DL.4k.H265.60fps.10bit.AAC-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&info, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("7"), "{notes:?}");
    // REMUX 仍按原盘归类，不改判 Encode
    let remux = ReleaseInfo {
        name: "Show.WEB-DL.REMUX.4k.H265-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, _n) = build_upload_fields(&remux, form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("3"));
}

#[test]
fn hdtv_alias_matches_tv_label() {
    // aling/ptcafe 实测：媒介选项写「TV」而不是「HDTV」
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="4">TV</option><option value="5">WEB-DL</option></select>"#;
    let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel"), Some("4"), "{notes:?}");
}

#[test]
fn bare_resolution_labels_match() {
    // ptsbao 实测：分辨率选项是 480/576/720/1080/2K/4K 裸数字
    let form = r#"<select name="standard_sel[1]"><option value="0">请选择一项</option><option value="1">1080</option><option value="2">720</option></select>"#;
    let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
    assert_eq!(field_lookup(&fs, "standard_sel"), Some("1"), "{notes:?}");
}

#[test]
fn multi_group_selects_all_filled() {
    // ptba 实测：medium_sel[4] 与 medium_sel[9] 是两套分类分支，都要独立配对填上
    let form = r#"<select name="medium_sel[4]"><option value="0">请选择</option><option value="7">HDTV</option></select>
<select name="medium_sel[9]"><option value="0">请选择</option><option value="3">HDTV</option><option value="8">Remux</option></select>"#;
    let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
    assert_eq!(field_lookup(&fs, "medium_sel[4]"), Some("7"));
    assert_eq!(field_lookup(&fs, "medium_sel[9]"), Some("3"), "{notes:?}");
}

#[test]
fn duplicate_exact_options_pick_first() {
    // muxuege 实测：两个同名 AAC（6 普通编码组 / 14 全景声同组），按文档顺序取首个普通组
    let form = r#"<select name="audiocodec_sel[4]"><option value="0">请选择一项</option><option value="6">AAC</option><option value="14">AAC</option></select>"#;
    let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
    assert_eq!(field_lookup(&fs, "audiocodec_sel").unwrap_or("0"), "6");
    assert!(
        !notes.iter().any(|n| n.contains("多个")),
        "同名首个已选中，不该再报歧义：{notes:?}"
    );
}

#[test]
fn region_selects_match_without_quality_note() {
    // aling/ubits 实测：source_sel 是地区下拉，按地区词配对；HDTV 不往里配，也不点名「没配对」
    let form = r#"<select name="source_sel[4]"><option value="0">请选择一项</option><option value="1">内地</option><option value="2">香港</option></select>"#;
    let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
    assert_eq!(field_lookup(&fs, "source_sel").unwrap_or("0"), "1");
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn off_domain_selects_not_core_gaps() {
    // soulvoice 实测：audiocodec_sel 选项是电子书格式（PDF/MOBI），不算音频质量缺口
    let prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://x/upload.php".into(),
            fields: vec![
                ("name".into(), "T".into()),
                ("descr".into(), "d".into()),
                ("technical_info".into(), "m".into()),
                ("type".into(), "402".into()),
                ("audiocodec_sel[8]".into(), "0".into()),
            ],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: r#"<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<select name="audiocodec_sel[8]"><option value="0">请选择</option><option value="1">PDF</option><option value="2">MOBI</option></select>"#
                .into(),
            poster_field: None,
            screenshot_field: None,
        };
    let info = ReleaseInfo {
        name: "T".into(),
        ..Default::default()
    };
    let d = analyze_form("sv", "声之形", &prep, &info);
    assert!(
        d.core_gaps().iter().all(|g| !g.contains("audiocodec")),
        "{:?}",
        d.core_gaps()
    );
    assert!(d.soft_gaps().iter().any(|g| g.contains("audiocodec_sel")));
}

#[test]
fn vertical_site_type_is_soft_not_core() {
    // haitang/kelu 实测：曲艺/垂直站分类里没有影视类，样本不适配不算表单缺口
    let form = r#"<select name="type"><option value="0">请选择</option><option value="4097">戏曲</option><option value="1">相声</option></select>"#;
    let (fs, _p, notes) = build_upload_fields(&drill_release(), form, &[]);
    assert_eq!(field_lookup(&fs, "type").unwrap_or("0"), "0");
    assert!(notes.iter().any(|n| n.contains("分类")), "{notes:?}");
    let prep = PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields: fs,
        picked_tags: vec![],
        tag_warn: None,
        select_notes: notes,
        required_gaps: vec![],
        form_html: form.into(),
        poster_field: None,
        screenshot_field: None,
    };
    let d = analyze_form("ht", "海棠", &prep, &drill_release());
    assert!(
        d.core_gaps().iter().all(|g| !g.contains("type")),
        "{:?}",
        d.core_gaps()
    );
    assert!(d.soft_gaps().iter().any(|g| g.contains("分类")));
}

#[test]
fn meta_tags_are_not_form_fields() {
    // hdcity 实测：<meta name="description"> 不能当成简介字段
    let page = r#"<meta name="description" content="HDCiTY is a platform">
<form action="takeupload.php"><input type="file" name="file"><input type="hidden" name="uid" value="1"></form>"#;
    assert!(!form_has(page, "description"));
    assert!(form_has(page, "file"));
    assert!(form_has(page, "uid"));
}

#[test]
fn gazelle_field_names_fallback() {
    // dicmusic 实测：Gazelle 架构用 title/subtitle/release_desc
    let form = r#"<form action="upload.php"><input type="file" name="file_input">
<input type="text" name="title"><input type="text" name="subtitle">
<textarea name="release_desc"></textarea></form>"#;
    let (fs, _p, _n) = build_upload_fields(&drill_release(), form, &[]);
    let f = |k: &str| field_lookup(&fs, k).unwrap_or("").to_string();
    assert_eq!(
        f("title"),
        "The.Disguiser.Complete.1080p.HDTV.x264.AAC-JING"
    );
    assert_eq!(
        f("subtitle"),
        "国产剧--伪装者[无台标完整版 全41集1080P][中字]"
    );
    assert!(
        f("release_desc").contains("MediaInfo"),
        "MediaInfo 内嵌简介"
    );
}

#[test]
fn missing_title_field_is_note_not_gap() {
    // tjupt 实测：表单没有标题输入框，标题由种子文件名自动生成
    let form = r#"<form action="takeupload.php"><input type="file" name="file">
<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<textarea name="descr"></textarea></form>"#;
    let (fs, _p, _n) = build_upload_fields(&drill_release(), form, &[]);
    assert!(field_lookup(&fs, "name").is_none());
    let prep = PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields: fs,
        picked_tags: vec![],
        tag_warn: None,
        select_notes: _n,
        required_gaps: vec![],
        form_html: form.into(),
        poster_field: None,
        screenshot_field: None,
    };
    let d = analyze_form("tjupt", "北洋", &prep, &drill_release());
    assert!(d.title_auto);
    assert!(
        d.core_gaps().iter().all(|g| !g.contains("标题")),
        "{:?}",
        d.core_gaps()
    );
}

#[test]
fn run_reseed_refuses_when_publish_gate_closed() {
    // 总闸关闭时必须在任何网络动作之前 bail（CLI/GUI/autopost 共用这道闸）
    let dir = std::env::temp_dir().join(format!("bsend-reseed-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("boxsend.json");
    std::fs::write(
            &cfg_path,
            r#"{
              "sourceSites": [{"id": "luckpt", "name": "LuckPT", "url": "https://pt.luckpt.de/", "framework": "NexusPHP", "enabled": true}],
              "targetSites": ["luckpt"],
              "downloader": {"url": "http://127.0.0.1:1", "username": "u", "password": "p"},
              "assist": {"publishEnabled": false}
            }"#,
        )
        .unwrap();
    let cfg = crate::config::Config::load(&cfg_path).unwrap();
    let qb = crate::qbittorrent::QBittorrent::new(&cfg.downloader.url, "u", "p").unwrap();
    let mut index = crate::index::ContentIndex::load(&cfg.coverage_state_path());
    let req = request_from_draft(
        "m:gate",
        "Gate Test",
        "luckpt",
        "https://pt.luckpt.de/details.php?id=1",
        "luckpt",
    );
    let err = run_reseed(&cfg, &qb, &req, &mut index, &mut |_| {}, None).unwrap_err();
    assert!(
        format!("{err:#}").contains("publishEnabled"),
        "错误必须点明总闸：{err:#}"
    );
}

#[test]
fn review_render_masks_secret_field_values() {
    let prepared = PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields: vec![
            ("name".into(), "Show.Name.2026".into()),
            ("passkey".into(), "abc123".into()),
            ("c_secure_pass".into(), "deadbeef".into()),
            ("descr".into(), "简介".into()),
        ],
        picked_tags: vec![],
        tag_warn: None,
        select_notes: vec![],
        required_gaps: vec![],
        form_html: String::new(),
        poster_field: None,
        screenshot_field: None,
    };
    let rep = ReviewReport {
        info: ReleaseInfo::default(),
        preflight: Preflight::default(),
        prepared,
        audit: None,
        audit_note: None,
        carry: None,
    };
    let text = rep.render(200);
    assert!(text.contains("name = Show.Name.2026"), "{text}");
    assert!(text.contains("passkey = <已隐藏 len=6>"), "{text}");
    assert!(text.contains("descr = 简介"), "{text}");
    assert!(!text.contains("abc123"), "passkey 值泄漏：{text}");
    assert!(!text.contains("deadbeef"), "secure 值泄漏：{text}");
    // dump_fields 同样走脱敏
    let dump = dump_fields(&[
        ("passkey".into(), "abc123".into()),
        ("name".into(), "Show".into()),
    ]);
    assert!(!dump.contains("abc123"), "{dump}");
    assert!(dump.contains("name=Show"), "{dump}");
}

#[test]
fn form_action_prefers_upload_form_over_nav_search() {
    // 导航搜索表单排在前面，上传表单在后：action 必须取上传块里的 takeupload.php，
    // 不能整页取第一个 <form> 误配成 search.php（tangpt/longpt 实机故障）
    let base = Url::parse("https://x.example/")
        .unwrap()
        .join("upload.php")
        .unwrap();
    let page = r#"<form action="search.php"><input name="search"><select name="search_area"><option value="1">全站</option></select></form>
<form action="takeupload.php" method="post" enctype="multipart/form-data"><input type="file" name="torrent"></form>"#;
    assert_eq!(
        form_action(page, &base).as_deref(),
        Some("https://x.example/takeupload.php")
    );
}

#[test]
fn form_action_keeps_offsite_absolute_endpoint() {
    // 城市类站点把种子 POST 到独立域名：站外绝对地址照收，不按本站根解析
    let base = Url::parse("https://city.example/upload.php").unwrap();
    let page = r#"<form action="https://up.other-domain.cc/takeupload.php" enctype="multipart/form-data"><input type="file" name="torrent"></form>"#;
    assert_eq!(
        form_action(page, &base).as_deref(),
        Some("https://up.other-domain.cc/takeupload.php")
    );
}

#[test]
fn non_upload_endpoint_blocks_navigation_pages() {
    let base = Url::parse("https://x.example/upload.php").unwrap();
    for bad in [
        "search.php",
        "bonusrain.php",
        "login.php",
        "index.php",
        "torrents.php",
        "browse.php",
        "details.php",
    ] {
        let action = resolve_url(bad, &base);
        assert!(
            non_upload_endpoint(&action, &base).is_some(),
            "{bad} 必须被拦截"
        );
    }
    // 上传端点与站外绝对地址放行
    assert!(non_upload_endpoint("https://x.example/takeupload.php", &base).is_none());
    assert!(non_upload_endpoint("https://up.other.cc/takeupload.php", &base).is_none());
}

#[test]
fn drill_select_options_excludes_nav_search() {
    let page = r#"<form action="search.php"><select name="search_area"><option value="1">全站</option></select></form>
<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="torrent">
<select name="type"><option value="0">请选择</option><option value="404">剧集</option></select>
<select name="medium_sel[4]"><option value="0">请选择</option><option value="7">HDTV</option></select></form>"#;
    let opts = drill_select_options(page);
    assert_eq!(opts.len(), 2, "{opts:?}");
    assert_eq!(opts[0].0, "type");
    assert!(opts[0].1.contains(&("404".to_string(), "剧集".to_string())));
    assert_eq!(opts[1].0, "medium_sel[4]");
    assert!(opts[1].1.contains(&("7".to_string(), "HDTV".to_string())));
}

fn prep_from_form(form: &str) -> PreparedUpload {
    let (fields, picked, notes) = build_upload_fields(&drill_release(), form, &[]);
    PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields,
        picked_tags: picked,
        tag_warn: None,
        select_notes: notes,
        required_gaps: vec![],
        form_html: form.into(),
        poster_field: None,
        screenshot_field: None,
    }
}

#[test]
fn multi_picks_checkboxes_match_and_fill() {
    // 标签归一命中（含别名/子串）→ 按站点真实 value 勾上；必填缺口清空
    let form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<input type="checkbox" name="style_sel[4][]" value="23"> 中字
<input type="checkbox" name="style_sel[4][]" value="24"> 国语配音
</form>"#;
    let mut prep = prep_from_form(form);
    let mut multi = std::collections::BTreeMap::new();
    multi.insert(
        "style_sel".to_string(),
        vec!["中字".to_string(), "国语".to_string()],
    );
    let rules = crate::gates::SiteRules {
        multi_picks: multi,
        required_fields: vec!["style_sel".into()],
        ..Default::default()
    };
    apply_site_rules(&mut prep, &rules, None);
    assert!(
        prep.fields
            .iter()
            .any(|(n, v)| n == "style_sel[4][]" && v == "23"),
        "{:?}",
        prep.fields
    );
    assert!(
        prep.fields
            .iter()
            .any(|(n, v)| n == "style_sel[4][]" && v == "24"),
        "{:?}",
        prep.fields
    );
    assert!(prep.required_gaps.is_empty(), "{:?}", prep.required_gaps);
}

#[test]
fn multi_picks_miss_keeps_empty_and_reports_required_gap() {
    // 标签配不上 → 不臆造勾选，保持未选；必填门禁点名未命中标签
    let form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<input type="checkbox" name="style_sel[4][]" value="23"> 内嵌字幕
<input type="checkbox" name="style_sel[4][]" value="24"> 粤语
</form>"#;
    let mut prep = prep_from_form(form);
    let mut multi = std::collections::BTreeMap::new();
    multi.insert(
        "style_sel".to_string(),
        vec!["中字".to_string(), "国语".to_string()],
    );
    let rules = crate::gates::SiteRules {
        multi_picks: multi,
        required_fields: vec!["style_sel".into()],
        ..Default::default()
    };
    apply_site_rules(&mut prep, &rules, None);
    assert!(
        !prep.fields.iter().any(|(n, _)| n.starts_with("style_sel")),
        "{:?}",
        prep.fields
    );
    assert_eq!(
        prep.required_gaps,
        vec!["必填缺失：style_sel[4][]（配置标签未命中：中字/国语）".to_string()]
    );
}

#[test]
fn required_tags_52_checkbox_auto_selected_or_blocked() {
    // 配置要求勾 ⛵方舟（value 52；13 是杜比、51 是合并，不能混勾）
    let form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<input type="checkbox" name="tags[4][]" value="52"> 方舟
<input type="checkbox" name="tags[4][]" value="13"> 杜比
</form>"#;
    let mut prep = prep_from_form(form);
    let rules = crate::gates::SiteRules {
        required_tags: vec![crate::gates::RequiredTag {
            field: "tags[4][]".into(),
            value: Some("52".into()),
            label: Some("方舟".into()),
        }],
        ..Default::default()
    };
    apply_site_rules(&mut prep, &rules, None);
    assert!(
        prep.fields
            .iter()
            .any(|(n, v)| n == "tags[4][]" && v == "52"),
        "{:?}",
        prep.fields
    );
    // 杜比（13）不是方舟，不能被当成必需标签勾上
    assert!(
        !prep
            .fields
            .iter()
            .any(|(n, v)| n == "tags[4][]" && v == "13"),
        "{:?}",
        prep.fields
    );
    assert!(prep.required_gaps.is_empty(), "{:?}", prep.required_gaps);

    // 红：页面上没有配置要求的 52 → 记缺口，门禁点名是哪个必需标签没勾
    let empty_form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file"></form>"#;
    let mut prep2 = prep_from_form(empty_form);
    apply_site_rules(&mut prep2, &rules, None);
    assert!(
        prep2
            .required_gaps
            .iter()
            .any(|g| g.contains("必需标签「方舟」未勾选")),
        "{:?}",
        prep2.required_gaps
    );
}

#[test]
fn image_split_counts_poster_first_rest_screenshots() {
    // 没有独立字段：第一张简介图算海报，其余算截图
    let prep = PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields: vec![(
            "descr".into(),
            "[img]https://a/1.jpg[/img]\n[img]https://a/2.jpg[/img]\n[img]https://a/3.jpg[/img]"
                .into(),
        )],
        picked_tags: vec![],
        tag_warn: None,
        select_notes: vec![],
        required_gaps: vec![],
        form_html: "<html><body><form></form></body></html>".into(),
        poster_field: None,
        screenshot_field: None,
    };
    assert_eq!(image_split_counts(&prep), (1, 2));

    // 有独立字段：海报字段 1 张，截图字段按行算
    let prep2 = PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields: vec![
            ("cover".into(), "https://a/1.jpg".into()),
            (
                "screenshots".into(),
                "https://a/2.jpg\nhttps://a/3.jpg".into(),
            ),
        ],
        picked_tags: vec![],
        tag_warn: None,
        select_notes: vec![],
        required_gaps: vec![],
        form_html: "<html><body><form></form></body></html>".into(),
        poster_field: Some("cover".into()),
        screenshot_field: Some("screenshots".into()),
    };
    assert_eq!(image_split_counts(&prep2), (1, 2));
}

#[test]
fn qingwa_bluray_variants_normalized_to_bluray() {
    for variant in [
        "Blu-ray", "Blu-Ray", "BLU-RAY", "Blu-RAY", "blu-Ray", "Blu ray", "BLURAY", "bluray",
    ] {
        let name = format!("Show 2024 1080p {variant} REMUX AVC-GRP");
        let got = normalize_title_for_site(&name, "qingwa");
        assert!(got.contains("BluRay"), "{variant} → {got}");
        assert!(
            !got.contains("Blu-ray") && !got.contains("Blu ray"),
            "{variant} → {got}"
        );
    }
    // 非青蛙站保持原样
    assert_eq!(
        normalize_title_for_site("Show 2024 Blu-ray 1080p", "sbpt"),
        "Show 2024 Blu-ray 1080p"
    );
}

#[test]
fn wanted_tags_detects_remux_in_any_case() {
    for name in [
        "Show 2024 Blu-ray REMUX 1080p AVC",
        "Show 2024 blu-ray remux 1080p AVC",
        "Show 2024 Remux 1080p AVC",
    ] {
        let info = ReleaseInfo {
            name: name.into(),
            ..Default::default()
        };
        assert!(wanted_tags(&info).contains(&"Remux"), "{name}");
    }
}

#[test]
fn all_images_pruned_sets_required_gap() {
    // 剪前有图、剪后 0 张：即使站点没配图数门槛，也必须留下阻断缺口。
    let mut prep = PreparedUpload {
            action: "https://x/takeupload.php".into(),
            form_url: "https://cangbao.ge/upload.php".into(),
            fields: vec![(
                "descr".into(),
                r#"<img src="https://i.imgur.com/a.jpg"><img src="https://i.imgur.com/b.jpg"><img src="https://hdsky.me/c.jpg">"#
                    .into(),
            )],
            picked_tags: vec![],
            tag_warn: None,
            select_notes: vec![],
            required_gaps: vec![],
            form_html: "<html><body><form></form></body></html>".into(),
            poster_field: None,
            screenshot_field: None,
        };
    apply_site_rules(&mut prep, &crate::gates::SiteRules::default(), None);
    assert_eq!(
        prep.required_gaps,
        vec!["简介图片全被剪掉（0 张可用）：先补可达的海报/截图再发布".to_string()]
    );
}

#[test]
fn required_single_select_empty_blocks() {
    // 必填单选（select / radio）未选 → 阻断；已选（checked/非 0 value）→ 不报缺口
    let select_form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<select name="region_sel[4]"><option value="0">请选择</option><option value="1">中国大陆</option></select>
</form>"#;
    let mut prep = prep_from_form(select_form);
    let rules = crate::gates::SiteRules {
        required_fields: vec!["region_sel".into()],
        ..Default::default()
    };
    apply_site_rules(&mut prep, &rules, None);
    assert_eq!(
        prep.required_gaps,
        vec!["必填缺失：region_sel[4]".to_string()]
    );

    let radio_empty = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<input type="radio" name="region_sel[4]" value="1"> 中国大陆
</form>"#;
    let mut prep2 = prep_from_form(radio_empty);
    apply_site_rules(&mut prep2, &rules, None);
    assert_eq!(
        prep2.required_gaps,
        vec!["必填缺失：region_sel[4]".to_string()]
    );

    let radio_checked = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<input type="radio" name="region_sel[4]" value="1" checked> 中国大陆
</form>"#;
    let mut prep3 = prep_from_form(radio_checked);
    apply_site_rules(&mut prep3, &rules, None);
    assert!(prep3.required_gaps.is_empty(), "{:?}", prep3.required_gaps);
}

#[test]
fn default_rules_leave_behavior_unchanged() {
    // 旧配置（无 multiPicks / requiredFields）：不写字段、不记缺口、不加提示
    let form = r#"<form action="takeupload.php" enctype="multipart/form-data"><input type="file" name="file">
<select name="type"><option value="0">请选择</option><option value="402">剧集</option></select>
<input type="checkbox" name="style_sel[4][]" value="23"> 中字
</form>"#;
    let mut prep = prep_from_form(form);
    let before = prep.fields.len();
    let notes_before = prep.select_notes.len();
    apply_site_rules(&mut prep, &crate::gates::SiteRules::default(), None);
    assert_eq!(prep.fields.len(), before);
    assert_eq!(prep.select_notes.len(), notes_before);
    assert!(prep.required_gaps.is_empty());
}

fn panda_descr_order() -> Vec<String> {
    vec![
        "【转载出处】".into(),
        "【制作说明】".into(),
        "【海报】".into(),
        "【简介】".into(),
        "【info】".into(),
        "【截图】".into(),
    ]
}

fn prep_with_descr(descr: &str, media: Option<&str>) -> PreparedUpload {
    let mut fields = vec![("descr".to_string(), descr.to_string())];
    if let Some(mi) = media {
        fields.push(("media_info".to_string(), mi.to_string()));
    }
    PreparedUpload {
        action: "https://x/takeupload.php".into(),
        form_url: "https://x/upload.php".into(),
        fields,
        picked_tags: vec![],
        tag_warn: None,
        select_notes: vec![],
        required_gaps: vec![],
        form_html: "<form></form>".into(),
        poster_field: None,
        screenshot_field: None,
    }
}

fn order_ctx(info: ReleaseInfo) -> DescrOrderCtx {
    DescrOrderCtx {
        source_label: "luckpt".into(),
        source_url: "https://luckpt.example/details.php?id=123".into(),
        target_label: "熊猫".into(),
        info,
    }
}

#[test]
fn descr_order_assembles_sections_when_source_has_no_markers() {
    let rules = crate::gates::SiteRules {
        descr_order: panda_descr_order(),
        require_images: Some(4),
        ..Default::default()
    };
    let info = ReleaseInfo {
        name: "The.Show.S01.1080p.WEB-DL.x264-GROUP".into(),
        subtitle: String::new(),
        descr: "源简介正文：本剧讲的是一个故事。IMDb：https://www.imdb.com/title/tt5154296/".into(),
        mediainfo: "General\nFormat : Matroska\nWidth : 1920 pixels".into(),
        imdb: Some("tt5154296".into()),
        douban: None,
        tmdb: None,
        completed_hint: false,
    };
    let shots = ["p1", "p2", "p3", "p4"];
    let imgs = shots
        .iter()
        .map(|p| format!("[img]https://img3.pixhost.to/images/1/{p}.jpg[/img]"))
        .collect::<Vec<_>>()
        .join("");
    let descr = format!("源简介正文：本剧讲的是一个故事。{imgs}");
    let mut prep = prep_with_descr(&descr, Some(info.mediainfo.as_str()));
    let ctx = order_ctx(info);
    apply_site_rules(&mut prep, &rules, Some(&ctx));

    assert!(prep.required_gaps.is_empty(), "{:?}", prep.required_gaps);
    let out = submitted_descr(&prep, "");
    // 六段齐全且顺序正确
    let (missing, ooo) = crate::gates::descr_order_gaps(out, &rules.descr_order);
    assert!(missing.is_empty() && ooo.is_empty(), "{out}");
    assert_eq!(crate::gates::image_count(out), 4);
    // 各段内容落位
    assert!(out.contains("luckpt https://luckpt.example/details.php?id=123"));
    assert!(out.contains("GROUP"));
    assert!(out.contains("本剧讲的是一个故事"));
    assert!(out.contains("Width : 1920 pixels"));
    // 海报是第一张，截图是其余三张；段标题只出现一次（幂等）
    let poster_pos = out.find("【海报】").unwrap();
    let shot_pos = out.find("【截图】").unwrap();
    assert!(poster_pos < shot_pos);
    assert!(out[poster_pos..].contains("p1.jpg"));
    assert!(out[shot_pos..].contains("p2.jpg"));
    assert_eq!(out.matches("【海报】").count(), 1);
}

#[test]
fn descr_order_keeps_compliant_descr_byte_identical() {
    let rules = crate::gates::SiteRules {
        descr_order: panda_descr_order(),
        require_images: Some(4),
        ..Default::default()
    };
    let compliant = concat!(
        "【转载出处】\nluckpt https://luckpt.example/details.php?id=123\n\n",
        "【制作说明】\nGROUP\n\n",
        "【海报】\n[img]https://img3.pixhost.to/images/1/p1.jpg[/img]\n\n",
        "【简介】\n本剧讲的是一个故事。\n\n",
        "【info】\nGeneral\nFormat : Matroska\n\n",
        "【截图】\n[img]https://img3.pixhost.to/images/1/p2.jpg[/img]",
        "[img]https://img3.pixhost.to/images/1/p3.jpg[/img]",
        "[img]https://img3.pixhost.to/images/1/p4.jpg[/img]"
    );
    let mut prep = prep_with_descr(compliant, Some("General\nFormat : Matroska"));
    let ctx = order_ctx(drill_release());
    apply_site_rules(&mut prep, &rules, Some(&ctx));
    // 已合规：原样保留，不重复包段、不重排
    assert_eq!(submitted_descr(&prep, ""), compliant);
}

#[test]
fn descr_order_skipped_when_rule_unconfigured() {
    // 没配 descrOrder 的站：装配函数直接返回，简介与字段逐字节不变。
    // 直接调装配函数，隔离掉既有的 HTML→BBCode 转换等其它规则步骤。
    let rules = crate::gates::SiteRules::default();
    let descr =
        "随便写的一段简介，[img]https://img3.pixhost.to/images/1/x.jpg[/img]没有任何段标题。";
    let mut prep = prep_with_descr(descr, None);
    let before = submitted_descr(&prep, "").to_string();
    let fields_before = prep.fields.clone();
    let ctx = order_ctx(drill_release());
    assemble_descr_by_order(&mut prep, &rules, Some(&ctx));
    assert_eq!(submitted_descr(&prep, ""), before);
    assert_eq!(prep.fields, fields_before);
}

#[test]
fn descr_order_missing_images_or_mediainfo_blocks_instead_of_force_post() {
    let rules = crate::gates::SiteRules {
        descr_order: panda_descr_order(),
        require_images: Some(4),
        ..Default::default()
    };
    // 图不够：2 张 < 4 → 阻断；装配被退回，简介保持原样，不带缺段简介提交。
    let descr = "只有两张图。[img]https://img3.pixhost.to/images/1/a.jpg[/img][img]https://img3.pixhost.to/images/1/b.jpg[/img]";
    let mut prep = prep_with_descr(descr, Some("General\nFormat : Matroska"));
    let ctx = order_ctx(drill_release());
    apply_site_rules(&mut prep, &rules, Some(&ctx));
    assert!(!prep.required_gaps.is_empty(), "{:?}", prep.required_gaps);
    assert!(prep.required_gaps.iter().any(|g| g.contains("图片")));
    let out = submitted_descr(&prep, "");
    assert_eq!(out, descr);
    assert!(!out.contains("【转载出处】"));

    // MediaInfo 缺失 → 阻断，不硬发。
    let four = ["p1", "p2", "p3", "p4"]
        .iter()
        .map(|p| format!("[img]https://img3.pixhost.to/images/1/{p}.jpg[/img]"))
        .collect::<Vec<_>>()
        .join("");
    let mut info = drill_release();
    info.mediainfo = String::new();
    let mut prep2 = prep_with_descr(&four, None);
    let ctx2 = order_ctx(info);
    apply_site_rules(&mut prep2, &rules, Some(&ctx2));
    assert!(
        prep2.required_gaps.iter().any(|g| g.contains("MediaInfo")),
        "{:?}",
        prep2.required_gaps
    );
}

#[test]
fn body_detail_success_url_kufei_style() {
    // kufei 类站点：提交后不发 Location 跳转，回包正文挂新种详情页唯一链接
    let action = "https://x.example/takeupload.php";
    let body = r#"<html><body><p>种子提交成功，请在个人主页查看</p>
<a href="details.php?id=12345">查看新种</a></body></html>"#;
    let u = body_detail_success_url(body, action, action);
    assert_eq!(u.as_deref(), Some("https://x.example/details.php?id=12345"));

    // 校验失败回显的上传表单（multipart）不参与成功识别
    let rerender = r#"<form action="takeupload.php" method="post" enctype="multipart/form-data">
<p>请填写必填项目</p><a href="details.php?id=999">旧种链接</a></form>"#;
    assert!(body_detail_success_url(rerender, action, action).is_none());

    // 回包里多个详情链接（侧边栏/推荐位）不能唯一定位，不算成功
    let two = r#"<html><body>
<a href="details.php?id=111">A</a><a href="details.php?id=222">B</a></body></html>"#;
    assert!(body_detail_success_url(two, action, action).is_none());

    // 用户主页锚不是种子详情页：只有 userdetails 时不算唯一详情
    let user = r#"<html><body><a href="userdetails.php?id=777">上传者</a></body></html>"#;
    assert!(body_detail_success_url(user, action, action).is_none());
    // 用户主页 + 唯一详情 → 仍能定位到详情
    let user_and_detail = r#"<html><body><a href="userdetails.php?id=777">上传者</a>
<a href="details.php?id=12345">新种</a></body></html>"#;
    assert_eq!(
        body_detail_success_url(user_and_detail, action, action).as_deref(),
        Some("https://x.example/details.php?id=12345")
    );
}

#[test]
fn extract_posted_links_uses_matching_id_not_first_link() {
    // hdvideo 实测：成功页先出现新种，后面跟着相关旧种；下载直链只有新种。
    // 即使旧种链接排在前面，也必须按 id 配对，不能取第一条详情。
    let base = Url::parse("https://hdvideo.top/").unwrap();
    let body = r#"<p>发布成功！</p>
<a href="details.php?id=14469">相关旧种</a>
<a href="download.php?id=78856">下载</a>
<a href="details.php?id=78856">新种</a>
<a href="userdetails.php?id=999">上传者</a>"#;
    let links = extract_posted_links(body, "https://hdvideo.top/takeupload.php", &base);
    assert_eq!(
        links.detail.as_deref(),
        Some("https://hdvideo.top/details.php?id=78856")
    );
    assert_eq!(
        links.download.as_deref(),
        Some("https://hdvideo.top/download.php?id=78856")
    );
}

#[test]
fn extract_posted_links_refuses_mismatched_ids() {
    // 详情指向 A、下载指向 B：分不清哪个是新种，宁可留空转人工，也不挂错种。
    let base = Url::parse("https://x.example/").unwrap();
    let body = r#"<a href="details.php?id=111">A</a><a href="download.php?id=222">B</a>"#;
    let links = extract_posted_links(body, "https://x.example/takeupload.php", &base);
    assert!(links.detail.is_none());
    assert!(links.download.is_none());
}

#[test]
fn tracker_check_allows_same_site_aliases_but_rejects_foreign() {
    let site = SiteConf {
        id: "hdvideo".into(),
        name: "HDVideo".into(),
        url: "https://hdvideo.top/".into(),
        announce_hosts: vec!["hdvideo.top".into()],
        announce_url: None,
        poster_field: None,
        screenshot_field: None,
        proxy: None,
        framework: crate::sites::Framework::NexusPHP,
        search_url: None,
        api_key: None,
    };
    let same_site = vec![
        "https://tracker.hdvideo.top/announce.php?passkey=a".to_string(),
        "https://hdvideo.top/announce.php?passkey=a".to_string(),
    ];
    assert!(ensure_trackers_on_site(&site, &same_site).is_ok());
    let foreign = vec!["https://tracker.other.example/announce".to_string()];
    assert!(ensure_trackers_on_site(&site, &foreign).is_err());
    assert!(ensure_trackers_on_site(&site, &[]).is_err());
}

#[test]
fn unresolved_after_site_persists_review_pending_not_failed() {
    let path = std::env::temp_dir().join(format!(
        "bsend-unresolved-{}-{}.json",
        std::process::id(),
        now_unix()
    ));
    let seed = serde_json::json!({
        "contents": {
            "m:test": {
                "name": "x",
                "totalSize": 1,
                "fileCount": 0,
                "manifestDigest": "",
                "torrents": {},
                "trackerHosts": [],
                "firstSeen": 0,
                "goneSince": null,
                "coverage": {},
                "source": null,
                "forbidReseed": false
            }
        }
    });
    std::fs::write(&path, seed.to_string()).unwrap();
    let mut index = ContentIndex::load(&path);
    let out = unresolved_after_site(
        &mut index,
        "m:test",
        "hdvideo",
        "posted_unresolved",
        Some("https://hdvideo.top/details.php?id=1".into()),
        "站上已发布，但挂回 qB 失败",
    )
    .unwrap();
    match out {
        ReseedOutcome::Duplicate(d) => {
            assert_eq!(d.status, CoverageStatus::ReviewPending);
            assert!(d.note.starts_with("posted_unresolved"));
            assert!(d.note.contains("勿重发"));
        }
        other => panic!("expected duplicate outcome, got {other:?}"),
    }
    let reloaded = ContentIndex::load(&path);
    let cell = &reloaded.contents["m:test"].coverage["hdvideo"];
    assert_eq!(cell.status, CoverageStatus::ReviewPending);
    assert!(cell
        .note
        .as_deref()
        .unwrap_or("")
        .starts_with("posted_unresolved"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn same_hash_skip_after_push_is_not_reported_as_posted() {
    // 2026-11-22 panda 10265 的现场：上传成功、qB 同 hash 但不是只挂熊猫的种，
    // 旧代码把 same_hash_skip 包成 posted，日志看起来发布成功、实际没做种。
    let out = same_hash_skip_after_push("https://pandapt.net/details.php?id=10265".into());
    assert!(matches!(
        out,
        ReseedOutcome::Duplicate(ref d) if d.status == CoverageStatus::SameHashSkip
    ));
    assert_eq!(out.audit_result(), "same_hash_skipped");
    assert!(out.display().contains("同 hash 跳过"), "{}", out.display());
    let ReseedOutcome::Duplicate(d) = &out else {
        unreachable!()
    };
    assert!(d.note.contains("当前没做种"), "{}", d.note);
    assert!(d.note.contains("待人工处理"), "{}", d.note);
}

#[test]
fn verifying_outcome_is_not_displayed_as_complete() {
    let out = ReseedOutcome::PostedVerifying("https://example.test/details.php?id=1".into());
    assert_eq!(out.audit_result(), "posted_verifying");
    let text = out.display();
    assert!(!text.contains("转种完成"), "{text}");
    assert!(text.contains("校验还没跑完"), "{text}");
    assert!(text.contains("不能算已在做种"), "{text}");
}

#[test]
fn seeded_summary_uses_mounted_flag_not_note_text() {
    fn dup(status: CoverageStatus, note: &str, mounted: bool) -> Result<ReseedOutcome> {
        Ok(ReseedOutcome::Duplicate(DuplicateOutcome {
            mounted,
            status,
            url: None,
            note: note.to_string(),
        }))
    }

    assert!(outcome_seeded(&Ok(ReseedOutcome::Posted(
        "https://example.test/details.php?id=1".into()
    ))));
    assert!(!outcome_seeded(&Ok(ReseedOutcome::PostedVerifying(
        "https://example.test/details.php?id=1".into()
    ))));
    assert!(!outcome_seeded(&dup(
        CoverageStatus::PendingReview,
        "站上已发布，等站点审核；审完前拿不到站点种子，没挂回 qB",
        false
    )));
    assert!(outcome_seeded(&dup(
        CoverageStatus::PendingReview,
        "站上已发布，种子已挂回 qB 做种；站点页面还标着“待审核”，等审核结果",
        true
    )));
    assert!(outcome_seeded(&dup(
        CoverageStatus::ReviewRejected,
        "站点审核没过：缺少海报；种子已挂回 qB，仍在做种",
        true
    )));
    // note 里写“已挂回 qB”但结构化标记是 false：以标记为准。
    assert!(!outcome_seeded(&dup(
        CoverageStatus::PendingReview,
        "这句话里写了已挂回 qB，但实际没挂回",
        false
    )));
}

#[test]
fn review_hold_and_seeding_outcomes_persist_distinct_cells() {
    let path = std::env::temp_dir().join(format!(
        "bsend-review-hold-{}-{}.json",
        std::process::id(),
        now_unix()
    ));
    let entry = serde_json::json!({
        "name": "x",
        "totalSize": 1,
        "fileCount": 0,
        "manifestDigest": "",
        "torrents": {},
        "trackerHosts": [],
        "firstSeen": 0,
        "goneSince": null,
        "coverage": {},
        "source": null,
        "forbidReseed": false
    });
    let seed = serde_json::json!({ "contents": { "m:held": entry, "m:seed": entry.clone(), "m:ok": entry } });
    std::fs::write(&path, seed.to_string()).unwrap();
    let mut index = ContentIndex::load(&path);
    let mut log = |_l: &str| {};
    let url = "https://www.hdkyl.in/details.php?id=479362";

    // 站点收下了但不给下载（等审核）：落“等站点审核”，不是 posted_unresolved、不算失败
    let out = review_hold_outcome(
        &mut index,
        "m:held",
        "qilin",
        Some(url.to_string()),
        ReviewMark::Pending,
        "尚未通过审核，请稍后再试",
        &mut log,
    )
    .unwrap();
    assert_eq!(out.audit_result(), "pending_review");
    let ReseedOutcome::Duplicate(d) = &out else {
        panic!("expected duplicate outcome, got {out:?}");
    };
    assert_eq!(d.status, CoverageStatus::PendingReview);
    assert!(!d.note.starts_with("posted_unresolved"), "{}", d.note);
    assert!(d.note.contains("尚未通过审核"), "{}", d.note);
    let cell = &index.contents["m:held"].coverage["qilin"];
    assert_eq!(cell.status, CoverageStatus::PendingReview);
    assert_eq!(cell.url.as_deref(), Some(url));

    // 能下载、也挂回 qB 了，但页面标着不通过：落“站点审核没过”，审计词 review_rejected
    let out = review_seeding_outcome(
        &mut index,
        "m:seed",
        "qilin",
        url,
        ReviewMark::Rejected,
        "审核不通过，原因：缺少海报或截图（图片少于 2 张）",
        &mut log,
    )
    .unwrap();
    assert_eq!(out.audit_result(), "review_rejected");
    let cell = &index.contents["m:seed"].coverage["qilin"];
    assert_eq!(cell.status, CoverageStatus::ReviewRejected);
    assert!(cell
        .note
        .as_deref()
        .is_some_and(|n| n.contains("缺少海报或截图")));

    // 页面标着“通过”＝正常发布完成：返回已发布，格子不动
    let out = review_seeding_outcome(
        &mut index,
        "m:ok",
        "qilin",
        url,
        ReviewMark::Approved,
        "通过",
        &mut log,
    )
    .unwrap();
    assert!(matches!(out, ReseedOutcome::Posted(ref u) if u == url));
    assert!(!index.contents["m:ok"].coverage.contains_key("qilin"));

    // “通过”不算审核卡住：站点标着通过却不给下载，走普通的下载失败路径
    let notice = |mark, why: &str| {
        anyhow::Error::new(SiteNotice {
            text: "提示".to_string(),
            review: Some((mark, why.to_string())),
        })
    };
    assert_eq!(
        site_review_hold(&notice(ReviewMark::Pending, "尚未通过审核")).map(|(m, _)| m),
        Some(ReviewMark::Pending)
    );
    assert_eq!(
        site_review_hold(&notice(ReviewMark::Approved, "通过")),
        None
    );
    assert!(site_review_hold(&anyhow::anyhow!("HTTP 500")).is_none());

    index.save().unwrap();
    let reloaded = ContentIndex::load(&path);
    assert_eq!(
        reloaded.contents["m:held"].coverage["qilin"].status,
        CoverageStatus::PendingReview
    );
    assert_eq!(
        reloaded.contents["m:seed"].coverage["qilin"].status,
        CoverageStatus::ReviewRejected
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn extract_posted_links_prefers_final_redirect() {
    let base = Url::parse("https://x.example/").unwrap();
    let body = r#"<a href="details.php?id=111">旧种</a><a href="download.php?id=333">下载</a>"#;
    let links = extract_posted_links(body, "https://x.example/details.php?id=333", &base);
    assert_eq!(
        links.detail.as_deref(),
        Some("https://x.example/details.php?id=333")
    );
    assert_eq!(
        links.download.as_deref(),
        Some("https://x.example/download.php?id=333")
    );
}

#[test]
fn image_urls_read_html_and_bbcode_in_order() {
    let descr = r#"海报<img src="https://img.example/poster.jpg">截图[img]https://img.example/1.jpg[/img]<img src="https://img.example/poster.jpg">[IMG]https://img.example/2.jpg[/IMG]"#;
    assert_eq!(
        image_urls_from_descr(descr),
        vec![
            "https://img.example/poster.jpg",
            "https://img.example/1.jpg",
            "https://img.example/2.jpg"
        ]
    );
}

#[test]
fn image_fields_fill_cover_and_screenshots_when_page_has_them() {
    let page = r#"<input name="cover"><textarea name="screenshots"></textarea><input name="name">"#;
    let descr = r#"<img src="https://img.example/poster.jpg"><img src="https://img.example/1.jpg"><img src="https://img.example/2.jpg"><img src="https://img.example/3.jpg">"#;
    let mut fields: Vec<Field> = vec![("name".into(), "标题".into())];
    fill_image_fields(&mut fields, page, descr, Some("cover"), Some("screenshots"));
    let get = |n: &str| {
        fields
            .iter()
            .find(|(name, _)| name == n)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("cover"), Some("https://img.example/poster.jpg"));
    assert_eq!(
        get("screenshots"),
        Some("https://img.example/1.jpg\nhttps://img.example/2.jpg\nhttps://img.example/3.jpg")
    );
}

#[test]
fn image_fields_skip_missing_form_field() {
    let page = r#"<input name="name">"#;
    let descr = r#"<img src="https://img.example/poster.jpg">"#;
    let mut fields: Vec<Field> = vec![("name".into(), "标题".into())];
    fill_image_fields(&mut fields, page, descr, Some("cover"), Some("screenshots"));
    assert_eq!(fields.len(), 1);
}

#[test]
fn image_fields_clear_stale_values_when_final_descr_has_no_images() {
    let page = r#"<input name="cover"><textarea name="screenshots"></textarea><input name="name">"#;
    let mut fields: Vec<Field> = vec![
        ("name".into(), "标题".into()),
        ("cover".into(), "https://img.example/dead-poster.jpg".into()),
        (
            "screenshots".into(),
            "https://img.example/dead-1.jpg".into(),
        ),
    ];
    fill_image_fields(
        &mut fields,
        page,
        "简介剪完死图后没有图",
        Some("cover"),
        Some("screenshots"),
    );
    assert!(fields.iter().all(|(n, _)| n != "cover"));
    assert!(fields.iter().all(|(n, _)| n != "screenshots"));
}

#[test]
fn region_dropdown_matches_chinese_japanese_and_falls_back_to_other() {
    // iloli 实测地区下拉：大陆1/香港2/台湾3/欧美4/韩国5/日本6/印度7/俄国8/泰国11/其它13
    let form = r#"<select name="source_sel[4]">
<option value="0">请选择一项</option><option value="1">大陆 (CHN)</option><option value="2">香港 (HKG)</option>
<option value="3">台湾 (TWN)</option><option value="4">欧美 (West)</option><option value="5">韩国 (KOR)</option>
<option value="6">日本 (JPN)</option><option value="7">印度 (IND)</option><option value="8">俄国 (RUS)</option>
<option value="11">泰国 (THA)</option><option value="13">其它 (Other)</option></select>"#;
    let cn = ReleaseInfo {
        name: "Guo Chan Ju 2024 1080p WEB-DL x264-GRP".into(),
        subtitle: "国产剧 全40集".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&cn, form, &[]);
    assert_eq!(
        field_lookup(&fs, "source_sel"),
        Some("1"),
        "国产 → 大陆：{notes:?}"
    );

    let jp = ReleaseInfo {
        name: "Japanese Drama 2024 1080p WEB-DL-GRP".into(),
        subtitle: "日剧 全10集".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&jp, form, &[]);
    assert_eq!(
        field_lookup(&fs, "source_sel"),
        Some("6"),
        "日剧 → 日本：{notes:?}"
    );

    let unknown = ReleaseInfo {
        name: "Movie.2024.1080p.BluRay.x264-GRP".into(),
        subtitle: String::new(),
        descr: String::new(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&unknown, form, &[]);
    assert_eq!(
        field_lookup(&fs, "source_sel"),
        Some("13"),
        "归不出 → 其它：{notes:?}"
    );
}

#[test]
fn processing_year_select_matches_release_year_or_leaves_empty() {
    // muxuege 实测：processing_sel 是发行年代（2010=14 … 2015=19 … 2027=31）
    let form = r#"<select name="processing_sel[4]">
<option value="0">请选择一项</option><option value="14">2010</option><option value="19">2015</option>
<option value="20">2016</option><option value="31">2027</option></select>"#;
    let with_year = ReleaseInfo {
        name: "Some Movie 2015 1080p BluRay AAC-GRP".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&with_year, form, &[]);
    assert_eq!(
        field_lookup(&fs, "processing_sel"),
        Some("19"),
        "2015 → 年代档 19：{notes:?}"
    );

    let no_year = ReleaseInfo {
        name: "Some Movie 1080p BluRay AAC-GRP".into(),
        mediainfo: "General\nFormat : Matroska".into(),
        ..Default::default()
    };
    let (fs, _p, notes) = build_upload_fields(&no_year, form, &[]);
    assert_eq!(
        field_lookup(&fs, "processing_sel").unwrap_or("0"),
        "0",
        "抽不到年份留空，不硬填：{notes:?}"
    );
}

#[test]
fn formcheck_and_gatecheck_share_rules_and_conclusions() {
    // formcheck 与 gatecheck 必须加载同一套站点规则：同一份构造结果两边缺口结论一致。
    let form = r#"<form action="takeupload.php">
<select name="type"><option value="0">请选择</option><option value="401">电影</option></select>
<select name="region_sel[4]"><option value="0">请选择</option><option value="7">Other</option></select>
</form>"#;
    let mut picks = std::collections::BTreeMap::new();
    picks.insert("region_sel".to_string(), "7".to_string());
    let rules = crate::gates::SiteRules {
        select_picks: picks,
        required_fields: vec!["region_sel".into()],
        ..Default::default()
    };

    let mut prep1 = prep_from_form(form);
    apply_site_rules(&mut prep1, &rules, None);
    let drill = analyze_form("x", "测试站", &prep1, &drill_release());

    let mut prep2 = prep_from_form(form);
    apply_site_rules(&mut prep2, &rules, None);
    let gate = analyze_prepared("x", "测试站", &prep2, &drill_release());

    assert_eq!(drill.core_gaps(), gate.core_gaps());
    assert_eq!(drill.soft_gaps(), gate.soft_gaps());
    assert!(
        gate.core_gaps().iter().all(|g| !g.contains("region_sel")),
        "规则回填后必填下拉不该再报缺口：{:?}",
        gate.core_gaps()
    );
}

#[test]
fn all_site_title_badges_are_stripped_idempotently() {
    let core = "Some.Release.2024.1080p.BluRay.x265-GRP";
    let badges = [
        "[免费]",
        "[免費]",
        "[Free]",
        "[FREE]",
        "[2X免费]",
        "[2x免费]",
        "[2X免費]",
        "[2x免費]",
        "[2x]",
        "[2X]",
        "[50%]",
        "[促销]",
        "[Promotion]",
        "[HR]",
        "[官方]",
        "剩余时间：23时59分钟",
    ];
    for badge in badges {
        let with_badge = format!("{core} {badge}");
        let stripped = strip_site_title_badges(&with_badge);
        assert_eq!(stripped, core, "角标 {badge} 应从标题尾部裁掉");
        assert_eq!(
            strip_site_title_badges(stripped),
            core,
            "裁剪必须幂等：{badge}"
        );
    }
}

#[test]
fn first_imdb_hit_ignores_anchor_without_title_attribute() {
    let base = Url::parse("https://13city.org/").unwrap();
    let html = r#"<a href="details.php?id=99">Agatha Christies Poirot S01-S13 Collection</a>"#;
    assert!(
        first_imdb_hit(
            html,
            &base,
            "Agatha Christies Poirot S01-S13 Collection 1080p Bluray x265 10bit MNHD-FRDS"
        )
        .is_none(),
        "只有链接文字、没有发布名 title 时不能判重"
    );
}

#[test]
fn hidden_mode_rows_are_not_counted_as_gaps() {
    // xingtan 实测：328 个 mode_<分类id> 隐藏行，只有当前分类那行可见；
    // 静态演练只看可见行，隐藏行的空下拉不计缺口。
    let form = r#"<form action="takeupload.php">
<tr relation="mode_16" class="mode_16"><td><select name="source_sel[16]"><option value="0">请选择</option><option value="1">视频</option></select>
<select name="processing_sel[16]"><option value="0">请选择</option><option value="14">2010</option></select></td></tr>
<tr relation="mode_31" class="mode_31"><td><select name="source_sel[31]"><option value="0">请选择</option><option value="2">音频</option></select></td></tr>
</form>"#;
    let prep = PreparedUpload {
        action: "https://xingtan.one/takeupload.php".into(),
        form_url: "https://xingtan.one/upload.php".into(),
        fields: vec![
            ("type".into(), "0".into()),
            ("source_sel[16]".into(), "0".into()),
            ("processing_sel[16]".into(), "0".into()),
            ("source_sel[31]".into(), "0".into()),
        ],
        picked_tags: vec![],
        tag_warn: None,
        select_notes: vec![],
        required_gaps: vec![],
        form_html: form.into(),
        poster_field: None,
        screenshot_field: None,
    };
    let d = analyze_form("xingtan", "星潭", &prep, &drill_release());
    assert!(
        !d.open_selects
            .iter()
            .any(|n| n.starts_with("source_sel[16]")
                || n.starts_with("processing_sel[16]")
                || n.starts_with("source_sel[31]")),
        "隐藏分类行的下拉不该进缺口：{:?}",
        d.open_selects
    );
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("mode_") && n.contains("分类联动")),
        "应提示隐藏行按分类生效：{:?}",
        d.notes
    );
}
