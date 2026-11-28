//! 详情页标题的促销/剩余时间角标裁剪。
//!
//! 站点会在标题尾部追加 `[免费] 剩余时间：23时59分钟` 这类角标；比对 existed
//! 线索或站内候选前必须先裁掉，否则真重复会被误判成“标题不符”。

/// 裁掉尾部促销角标，返回标题正文前缀；找不到角标时返回去空白后的原串。
pub(super) fn strip_site_title_badges(title: &str) -> &str {
    const BADGES: [&str; 20] = [
        "[免费]",
        "[免費]",
        "[Free]",
        "[FREE]",
        "[2X免费]",
        "[2x免费]",
        "[2X免費]",
        "[2x免費]",
        "[2X 50%]",
        "[2x 50%]",
        "[2x]",
        "[2X]",
        "[50%]",
        "[促销]",
        "[Promotion]",
        "[HR]",
        "[官方]",
        "剩余时间",
        "剩餘時間",
        "剩余時間",
    ];
    // 裁在标题里最早出现的角标上，而不是内置列表里靠前的那个：
    // 标题是 `X [2X] [Free]` 时若先按列表顺序裁 [Free]，会留下 `X [2X]` 仍带着角标。
    let mut cuts: Vec<usize> = BADGES.iter().filter_map(|b| title.find(b)).collect();
    cuts.sort_unstable();
    cuts.dedup();
    for pos in cuts {
        // 位置 0 的角标没有标题前缀；落在另一个方括号内部的命中（如
        // `[2X免费]` 里的 `[免费]`）裁出来只会留下半截角标，跳过。
        if pos == 0 || title[..pos].ends_with('[') {
            continue;
        }
        let core = title[..pos].trim();
        if !core.is_empty() {
            return core;
        }
    }
    title.trim()
}

/// 标题比对归一化：小写，非 ASCII 字母数字统一成点，去掉空段。
pub(super) fn norm_post_title(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '.' })
        .collect::<String>()
        .split('.')
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}
