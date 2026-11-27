//! 上传页类型细分。
//!
//! 旧逻辑抓不到静态表单时一律报「没有可解析的静态上传表单（JS/自定义界面，或 cookie 失效）」，
//! 把权限不足、登录墙、Cloudflare/区域时段拦截、纯前端 SPA 混在一起。这里按页面特征先归类，
//! 让上层能给出点名的报错：缺什么、是要人工过验证还是干脆不支持自动上传。

/// 上传页归类结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadPageKind {
    /// 静态上传表单（文件框 / hidden / select / textarea 至少有一样），纯 HTTP 可构造字段。
    StaticForm,
    /// 已登录，但账号没有在该站发布种子的权限。
    NoPermission,
    /// 登录墙：cookie 已失效，页面是登录表单。
    LoginExpired,
    /// 二次验证中间页：站点要求先过 2FA（脚本跳转到 2FA 页面），纯 HTTP 拿不到表单。
    TwoFa,
    /// Cloudflare/人机验证或区域时段拦截：需要前台真实浏览器人工过一次验证。
    ChallengeGate,
    /// 纯前端 SPA / JSON API：表单控件由网页 JS 渲染，纯 HTTP 拿不到字段名与提交接口。
    SpaApi,
    /// 页面既没有静态表单，也认不出上述特征（兜底，沿用旧提示）。
    NoForm,
}

/// 权限不足的页面文案（简繁都收；PTT/TTG 实测写法各异）。
const DENIED: &[&str] = &[
    "没有发布种子的权限",
    "沒有發布種子的許可權",
    "沒有發佈種子的許可權",
    "没有发布权限",
    "沒有發布權限",
    "沒有發佈權限",
    "没有上传种子的权限",
    "沒有上傳種子的權限",
    "没有上传权限",
    "沒有上傳權限",
    "在该板块发布种子的权限",
    "在該板塊發佈種子的權限",
    "权限不足",
    "權限不足",
    "許可權不足",
    "等级不足",
    "等級不足",
    "不允许上传",
    "不允許上傳",
    "禁止上传",
    "禁止上傳",
];

/// 人机验证 / 区域时段拦截的页面特征。
const CHALLENGE: &[&str] = &[
    "cdn-cgi/challenge-platform",
    "challenges.cloudflare.com",
    "turnstile",
    "Checking your browser",
    "Access is prohibited",
    "禁止仿问",
    "禁止访问",
    "禁止訪問",
    "区域/时间禁止",
    "區域/時間禁止",
];

/// 二次验证中间页特征（hddolby 实测：表单前先脚本跳 take2fa.php）。
const TWOFA: &[&str] = &[
    "take2fa.php",
    "2fa.php",
    "两步验证",
    "二次验证",
    "两步身份验证",
    "two-factor",
];

/// 纯前端 SPA 外壳特征（rousi 实测：React Router，`ssr:false`，控件全部懒加载）。
const SPA: &[&str] = &["__reactRouterContext", "ssr\":false", "modulepreload"];

/// 从页面文案里命中的权限不足关键词；没命中返回 None。
pub fn permission_keyword(html: &str) -> Option<&'static str> {
    DENIED.iter().find(|kw| html.contains(*kw)).copied()
}

fn has_login_form(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("type=\"password\"") || lower.contains("type='password'")
}

fn has_challenge(html: &str) -> bool {
    CHALLENGE.iter().any(|m| html.contains(m))
}

fn has_twofa(html: &str) -> bool {
    TWOFA
        .iter()
        .any(|m| html.to_ascii_lowercase().contains(&m.to_ascii_lowercase()))
}

fn looks_like_spa(html: &str) -> bool {
    let hits = SPA.iter().filter(|m| html.contains(*m)).count();
    // modulepreload 只在带 /assets/ 包路径时才算 SPA 包，避免误伤普通静态站
    let has_assets = html.contains("modulepreload") && html.contains("/assets/");
    hits >= 2 || (hits >= 1 && has_assets)
}

fn has_static_form(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    // 只有 hidden/number 的捐款、搜索、登录表单不是上传表单。
    // 上传表单至少要能选种子文件或填简介文本域；只看到一个 hidden 不能算。
    lower.contains("<form")
        && (lower.contains("type=\"file\"")
            || lower.contains("type='file'")
            || lower.contains("<textarea"))
}

/// 按上传页 HTML 归类。判定顺序：登录墙 → 权限 → 静态表单 → 2FA → 验证/区域拦截 → SPA → 兜底。
/// 静态表单优先：挂了 CF 探测脚本但页面真有表单控件的站，仍按静态表单走。
pub fn classify_upload_page(html: &str) -> UploadPageKind {
    if has_login_form(html) {
        return UploadPageKind::LoginExpired;
    }
    if permission_keyword(html).is_some() {
        return UploadPageKind::NoPermission;
    }
    if has_static_form(html) {
        return UploadPageKind::StaticForm;
    }
    if has_twofa(html) {
        return UploadPageKind::TwoFa;
    }
    if has_challenge(html) {
        return UploadPageKind::ChallengeGate;
    }
    if looks_like_spa(html) {
        return UploadPageKind::SpaApi;
    }
    UploadPageKind::NoForm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_nexusphp_form_still_classified_static() {
        // hddolby 实测页面骨架：文件框 + 标题 + 简介 + MediaInfo + 下拉
        let html = r#"<form id="compose" action="takeupload.php" method="post" enctype="multipart/form-data">
<input type="file" name="file" /><input type="text" name="name" />
<textarea name="descr"></textarea><textarea name="media_info"></textarea>
<select name="type"><option value="1">Movie</option></select>
<input type="hidden" name="citeid" value="0" /></form>"#;
        assert_eq!(classify_upload_page(html), UploadPageKind::StaticForm);
    }

    #[test]
    fn permission_denial_wordings_recognized() {
        // TTG 实测：您没有上传种子的权限
        let ttg = "<h2>对不起，Sorry</h2>您没有上传种子的权限。You are not allowed to upload here.";
        assert_eq!(classify_upload_page(ttg), UploadPageKind::NoPermission);
        // PTT 实测：你没有在该板块发布种子的权限
        let ptt = "你没有在该板块发布种子的权限。相关提醒：先阅读发种规则";
        assert_eq!(classify_upload_page(ptt), UploadPageKind::NoPermission);
        assert!(permission_keyword(ttg).is_some());
        assert!(permission_keyword(ptt).is_some());
    }

    #[test]
    fn login_wall_classified_expired() {
        let html = r#"<form action="login.php"><input type="password" name="password" /></form>"#;
        assert_eq!(classify_upload_page(html), UploadPageKind::LoginExpired);
    }

    #[test]
    fn challenge_gate_not_treated_as_missing_form() {
        // mypt 实测：区域/时段禁止 + Cloudflare challenge-platform 脚本
        let html = r#"<html><title>URL Lmitation Permission Control</title>
<script src="/cdn-cgi/challenge-platform/scripts/jsd/main.js"></script>
<div>当前区域/时间禁止仿问……Access is prohibited in the current region/time zone</div></html>"#;
        assert_eq!(classify_upload_page(html), UploadPageKind::ChallengeGate);
    }

    #[test]
    fn spa_shell_classified_spa_api() {
        // rousi 实测骨架：React Router SPA，静态 HTML 里没有任何表单控件
        let html = r#"<html><head>
<link rel="modulepreload" href="/assets/v2/root-DfDCmhtg.js" />
<script>window.__reactRouterContext={"ssr":false,"isSpaMode":true};</script>
</head><body><main aria-label="加载中">skeleton</main></body></html>"#;
        assert_eq!(classify_upload_page(html), UploadPageKind::SpaApi);
    }

    #[test]
    fn twofa_interstitial_classified_separately() {
        // hddolby 实测：登录态在，但表单前先脚本跳 take2fa.php
        let html = r#"<table class="main">…公告…</table>
<script>window.location.href = "take2fa.php?returnto=%2Fupload.php";</script>"#;
        assert_eq!(classify_upload_page(html), UploadPageKind::TwoFa);
    }

    #[test]
    fn challenge_beacon_plus_real_form_stays_static() {
        // 页面挂了 CF jsd 探测脚本，但表单控件齐全：仍走静态表单路径
        let html = r#"<script src="/cdn-cgi/challenge-platform/scripts/jsd/main.js"></script>
<form action="takeupload.php" method="post"><input type="file" name="file" />
<input type="text" name="name" /><textarea name="descr"></textarea></form>"#;
        assert_eq!(classify_upload_page(html), UploadPageKind::StaticForm);
    }

    #[test]
    fn unknown_shell_falls_back_to_no_form() {
        let html = "<html><body><p>页面里既没有表单也认不出特征</p></body></html>";
        assert_eq!(classify_upload_page(html), UploadPageKind::NoForm);
    }

    #[test]
    fn donation_form_with_only_hidden_fields_is_not_an_upload_form() {
        // longpt 实测：upload.php 只有一个 bonusrain.php 捐款表单（hidden + number），
        // 不能因为有 hidden 就当成静态上传表单。
        let html = r#"<form action="bonusrain.php" method="post">
<input type="hidden" name="action" value="donate">
<input type="hidden" name="nonce" value="x">
<input type="number" name="amount"></form>"#;
        assert_eq!(classify_upload_page(html), UploadPageKind::NoForm);
    }
}
