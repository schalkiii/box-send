//! 发种门禁（G0–G3）：把站点规范变成可执行检查，供 `gatecheck` 与自动发种复用。
//!
//! 设计原则：**默认保守**。规则表没配的站点只自动发「分类/质量下拉全部唯一命中 +
//! 必填（简介/MediaInfo/链接）齐全 + 官方预审通过」的内容；任何一项不满足就 Block
//! （转人工），绝不放宽猜测。

use serde::{Deserialize, Serialize};

/// 单条门禁的结论
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Block,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Block => "block",
        }
    }
}

/// 一条门禁结果
#[derive(Debug, Clone)]
pub struct GateResult {
    pub gate: &'static str,
    pub verdict: Verdict,
    pub message: String,
}

/// 站点必须勾选的一个标签：字段名/前缀 + 复选框 value 或页面标签文案。
/// `value` 与 `label` 至少给一个；页面控件按任一命中即视为已勾上。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequiredTag {
    /// 复选框字段名/前缀（如 `tags[4][]`）
    pub field: String,
    /// 复选框 value（标签 id，如 agsvpt ⛵方舟 `52`）；与 label 任一命中即可
    pub value: Option<String>,
    /// 页面标签文案（日志/报错用）；与 value 任一命中即可
    pub label: Option<String>,
}

/// 分类下由页面 JS 异步注入的附加必填字段，取值口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CategoryFieldKind {
    /// 中文名：从副标题/简介抽中文片段；派生不出就记缺口，不发空值
    #[default]
    ChineseName,
    /// 英文名：取主标题；为空就记缺口，不发空值
    EnglishName,
    /// 剧集类型/地区：按标题线索归类，归不出用 `default`（兜底「其他」）
    SpecificCat,
    /// 地区复选框：按片段里的真实控件勾选，不臆造字段名
    Checkbox,
}

/// 分类下一个 JS 异步注入的附加必填字段（静态上传页里没有这个控件）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRequiredField {
    /// 表单字段名（以异步片段里的真实控件为准，不要凭空猜）
    pub field: String,
    pub kind: CategoryFieldKind,
    /// `SpecificCat` 归不出时的兜底值；`Checkbox` 的提交 value；其余类型不用
    pub default: Option<String>,
}

/// 某个分类（表单 `type` 值，如剧集 `402`）需要的附加必填字段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRule {
    /// 分类 value，与表单 `type` 字段值比对
    pub category: String,
    /// 分类控件定义片段的端点模板，`{catid}` 替换成分类值；
    /// 如 `showcategorydetail.php?catid={catid}`。拉到片段后以片段里的真实控件名为准。
    pub detail_endpoint: Option<String>,
    #[serde(default)]
    pub fields: Vec<CategoryRequiredField>,
}

/// 站点规范（`assist.reseedRules.<siteId>`；缺省全保守）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteRules {
    /// 标题最大长度（字符）；超过即 Block
    pub title_max_len: Option<usize>,
    /// 主标题禁止出现汉字（站点只收英文发布名；中文名放副标题）
    pub title_no_cjk: Option<bool>,
    /// 简介至少包含几张图片（BBCode `[img]` 或 HTML `<img>`，按最终提交值计）。
    /// **不读配置**：由 [`image_floor`] 按站点硬编码，`rules_for` 每次覆盖这个值。
    pub require_images: Option<usize>,
    /// 简介必须按这些标题/标记的顺序出现（如 `["【转载出处】","【制作说明】","【海报】","【简介】","【info】","【截图】"]`）
    #[serde(default)]
    pub descr_order: Vec<String>,
    /// 是否要求简介里带 IMDb/豆瓣/TMDB 链接（缺省 true）
    pub require_links: Option<bool>,
    /// 是否要求 MediaInfo 非空（缺省 true）
    pub require_mediainfo: Option<bool>,
    /// 禁发关键词（名称/副标题/简介命中即 Block）
    #[serde(default)]
    pub banned_keywords: Vec<String>,
    /// 允许的分类（为空 = 不限制）；与站点「分类」下拉文案比对
    #[serde(default)]
    pub allowed_types: Vec<String>,
    /// 逐站显式选档覆盖：字段前缀 → 选项 value（实测取证后配置，消歧用）
    #[serde(default)]
    pub select_picks: std::collections::BTreeMap<String, String>,
    /// 复选框/多选组显式勾选：字段前缀（如 `style_sel`）→ 要勾选的标签文案列表
    /// （如 `["中字","国语"]`）。按页面真实标签归一匹配；匹配不到不臆造、保持未选。
    #[serde(default)]
    pub multi_picks: std::collections::BTreeMap<String, Vec<String>>,
    /// 站点必须勾选的标签（由配置 `reseedRules.<siteId>.requiredTags` 声明；
    /// 如 agsvpt ⛵方舟：字段 `tags[4][]`、value `52`。注意 `13` 是杜比、`51` 是合并，
    /// 不要混）。构造完成后仍未选中即记缺口并阻断发布；按页面真实控件自动勾，
    /// 勾不上不臆造。代码层不预设"某站所有种子一律勾某个标签"，是否需要由配置决定。
    #[serde(default)]
    pub required_tags: Vec<RequiredTag>,
    /// 分类下 JS 异步注入的附加必填字段（按分类 value 声明；静态上传页没有这些控件）。
    /// 配置 `reseedRules.<siteId>.categoryRules` 可覆盖/追加；缺省走 [`category_rules_default`]。
    #[serde(default)]
    pub category_rules: Vec<CategoryRule>,
    /// 必填字段名前缀（如 `["region_sel","style_sel"]`）：任一前缀下没有已选值
    /// 即判核心缺口并阻断发布；未配置的站点行为不变。
    #[serde(default)]
    pub required_fields: Vec<String>,
    /// 简介海报最低张数（硬规则，由 [`image_floor_detail`] 按站点覆盖；None = 不要求拆分）
    pub require_poster: Option<usize>,
    /// 简介截图最低张数（硬规则，由 [`image_floor_detail`] 按站点覆盖；None = 不要求拆分）
    pub require_screenshots: Option<usize>,
    /// 简介格式：缺省 `bbcode` = 提交前把 HTML 转 BBCode（NexusPHP 的简介框是
    /// BBCode 编辑器，原样提交 HTML 会被转义成字面 `<fieldset>`）；显式 `html`
    /// 才保持 HTML 原样（仅用于确认支持 HTML 的站）。
    pub descr_format: Option<String>,
    /// 海报必须排在简介文字之前（青蛙、AGSVPT 实测：图在文末会被审核退回）。
    /// 开启后，提交前把简介里的第一张图移到最前面；已有多张图时只移动第一张，
    /// 其余截图顺序不变。
    pub poster_first: Option<bool>,
    /// 每日自动发种上限（缺省 2）
    pub daily_limit: Option<u32>,
    /// 上传请求超时秒数（缺省 120）：站点在服务端解 torrent、查重慢时可逐站放宽
    pub upload_timeout_sec: Option<u64>,
    /// 规则来源备注
    pub note: Option<String>,
}

impl SiteRules {
    pub fn require_links(&self) -> bool {
        self.require_links.unwrap_or(true)
    }
    pub fn require_mediainfo(&self) -> bool {
        self.require_mediainfo.unwrap_or(true)
    }
    pub fn daily_limit(&self) -> u32 {
        self.daily_limit.unwrap_or(2)
    }
    pub fn upload_timeout_sec(&self) -> u64 {
        self.upload_timeout_sec.unwrap_or(120).max(1)
    }
    /// 简介是否按 BBCode 提交：缺省是；只有显式 `html` 才保持 HTML。
    pub fn descr_is_bbcode(&self) -> bool {
        self.descr_format
            .as_deref()
            .is_none_or(|f| !f.eq_ignore_ascii_case("html"))
    }
}

/// 要图站点的图片底线：总图数 + 海报/截图拆分要求（硬规则，不读配置）。
/// `min_poster` / `min_screenshots` 为 0 表示该站不要求按独立字段拆分，只看总图数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImageFloor {
    pub min_total: usize,
    pub min_poster: usize,
    pub min_screenshots: usize,
}

/// 各站实测站规口径：
/// - 藏宝阁 / 优堡 / 太乙 / 麒麟：3 张（死图剪掉后不够就拦；只看总图数，不拆海报/截图）
/// - 熊猫：4 张（站规更严，且要求分段顺序；只看总图数）
/// - ptlgs：4 张 = 1 张海报 + 3 张截图（独立字段 `cover` / `screenshots`）
/// - agsvpt：1 张海报 + 1 张截图（缺海报或截图都会被审核退回）
/// - tjupt：至少 1 张图（管理要求补海报，按总图兜底，不拆海报/截图）
pub fn image_floor_detail(site_id: &str) -> Option<ImageFloor> {
    match site_id {
        "cangbao" | "ubits" | "tey" | "qilin" => Some(ImageFloor {
            min_total: 3,
            min_poster: 0,
            min_screenshots: 0,
        }),
        "panda" => Some(ImageFloor {
            min_total: 4,
            min_poster: 0,
            min_screenshots: 0,
        }),
        "ptlgs" => Some(ImageFloor {
            min_total: 4,
            min_poster: 1,
            min_screenshots: 3,
        }),
        "agsvpt" => Some(ImageFloor {
            min_total: 2,
            min_poster: 1,
            min_screenshots: 1,
        }),
        "tjupt" => Some(ImageFloor {
            min_total: 1,
            min_poster: 0,
            min_screenshots: 0,
        }),
        _ => None,
    }
}

/// 实测站规兜底的分类附加必填字段（配置 `categoryRules` 非空时以配置为准）。
/// tjupt 剧集分类（`type=402`）的 `cname`/`ename`/`specificcat` 是页面 JS
/// 从 `showcategorydetail.php?catid=402` 异步注入的，静态上传页里没有；不补就被
/// 服务端以“有项目没有填写”退回。字段名以异步片段为准，这里只存实测口径。
pub fn category_rules_default(site_id: &str) -> Vec<CategoryRule> {
    match site_id {
        "tjupt" => vec![CategoryRule {
            category: "402".into(),
            detail_endpoint: Some("showcategorydetail.php?catid={catid}".into()),
            fields: vec![
                CategoryRequiredField {
                    field: "cname".into(),
                    kind: CategoryFieldKind::ChineseName,
                    default: None,
                },
                CategoryRequiredField {
                    field: "ename".into(),
                    kind: CategoryFieldKind::EnglishName,
                    default: None,
                },
                CategoryRequiredField {
                    field: "specificcat".into(),
                    kind: CategoryFieldKind::SpecificCat,
                    default: Some("其他".into()),
                },
            ],
        }],
        _ => Vec::new(),
    }
}

/// 要图的站点最低图数：硬规则，不读配置。
/// 这些站的审核会逐张看图，少于门槛必被退回；`At least 3` 是 2026-11-24 定下的底线。
/// 熊猫高清站规另外明说“4 张图 + 段序”，比底线更严，保留 4，免得按 3 张发又被退。
/// ptlgs 上传页要求独立 `cover` 海报 + 至少 3 张 `screenshots`，所以按 4 张算总图数。
pub fn image_floor(site_id: &str) -> Option<usize> {
    image_floor_detail(site_id).map(|f| f.min_total)
}

/// 主标题里是否有汉字（CJK Unified Ideographs + 扩展 A + 兼容区）。
/// 只判汉字，不把中文标点/全角括号当“中文”，避免误伤英文标题里的装饰符号。
pub fn has_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c as u32,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        )
    })
}

/// 简介里图片数量：BBCode `[img]`/`[img=…]` 与 HTML `<img>` 都算。
/// 按最终提交值统计，所以剪死图后这里会同步减少。
pub fn image_count(descr: &str) -> usize {
    let lower = descr.to_ascii_lowercase();
    let mut count = 0usize;
    let mut from = 0usize;
    while let Some(p) = lower[from..].find("[img") {
        let at = from + p + 4;
        let next = lower.as_bytes().get(at).copied();
        if matches!(next, Some(b']') | Some(b'=')) {
            count += 1;
        }
        from = at;
    }
    from = 0;
    while let Some(p) = lower[from..].find("<img") {
        count += 1;
        from += p + 4;
    }
    count
}

/// 按规则要求的标题标记顺序检查简介；返回（缺失项，顺序错误项）。
/// 标记按字面匹配（可带中文方括号），这样正文里偶然出现的“简介”不会误判。
pub fn descr_order_gaps(descr: &str, order: &[String]) -> (Vec<String>, Vec<String>) {
    let mut missing = Vec::new();
    let mut out_of_order = Vec::new();
    let mut last = 0usize;
    for label in order {
        match descr.find(label) {
            None => missing.push(label.clone()),
            Some(pos) => {
                if pos < last {
                    out_of_order.push(label.clone());
                }
                last = pos;
            }
        }
    }
    (missing, out_of_order)
}

/// 门禁输入（来自审阅结果；不直接碰网络）
pub struct GateInput<'a> {
    pub rules: &'a SiteRules,
    pub name: &'a str,
    pub subtitle: &'a str,
    pub descr: &'a str,
    pub mediainfo: &'a str,
    pub has_links: bool,
    /// 站内分类文案（选中下拉的 label；None = 未选中）
    pub type_label: Option<&'a str>,
    /// 核心缺口（来自 analyze_form：type/medium/codec/standard/audiocodec 未选）
    pub core_gaps: &'a [String],
    /// 规则判定的必填字段缺口（`apply_site_rules` 写入；每条文案含「必填缺失」）；非空即 Block
    pub required_gaps: &'a [String],
    /// 海报张数（独立字段有值按字段算；否则第一张简介图算海报）
    pub poster_count: usize,
    /// 截图张数（独立字段按行算；否则其余简介图算截图）
    pub screenshot_count: usize,
    /// 官方预审结论：None = 站点无预审；Some(false) = 未通过
    pub audit_passed: Option<bool>,
}

/// 跑全部门禁；返回逐条结果。全部 Pass 才算通过。
pub fn evaluate(input: &GateInput<'_>) -> Vec<GateResult> {
    let mut out = Vec::new();
    let mut add = |gate: &'static str, ok: bool, msg: String| {
        out.push(GateResult {
            gate,
            verdict: if ok { Verdict::Pass } else { Verdict::Block },
            message: msg,
        });
    };

    // G1 分类：必须唯一命中
    let type_gap = input
        .core_gaps
        .iter()
        .any(|g| g == "type" || g == "type 未选");
    add(
        "G1 分类",
        !type_gap,
        if type_gap {
            "分类未选中（站内分类与样本类型不匹配或存在歧义）".into()
        } else {
            format!("分类={}", input.type_label.unwrap_or("已选中"))
        },
    );

    // G1 分类白名单（规则配置时）
    if !input.rules.allowed_types.is_empty() {
        let label = input.type_label.unwrap_or("");
        let ok = input
            .rules
            .allowed_types
            .iter()
            .any(|t| label.contains(t.as_str()));
        add(
            "G1 分类白名单",
            ok,
            if ok {
                format!("分类「{label}」在白名单内")
            } else {
                format!(
                    "分类「{label}」不在白名单（{}）",
                    input.rules.allowed_types.join("/")
                )
            },
        );
    }

    // G2 质量档：核心缺口里除 type 之外的一项都不许有
    let quality_gaps: Vec<&String> = input
        .core_gaps
        .iter()
        .filter(|g| *g != "type" && g.as_str() != "type 未选")
        .collect();
    add(
        "G2 质量档",
        quality_gaps.is_empty(),
        if quality_gaps.is_empty() {
            "分类/媒介/编码/分辨率/音频 全部命中".into()
        } else {
            format!(
                "质量下拉有缺口：{}",
                quality_gaps
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("；")
            )
        },
    );

    // G3 必填字段 / 必需标签（规则配置的单选/复选组；已有本地缺口时也参与判定，
    // 例如分类异步字段派生不出值，缺口不能被门禁吞掉）
    if !input.rules.required_fields.is_empty()
        || !input.rules.required_tags.is_empty()
        || !input.required_gaps.is_empty()
    {
        add(
            "G3 必填字段",
            input.required_gaps.is_empty(),
            if input.required_gaps.is_empty() {
                "必填字段与必需标签均已选中".into()
            } else {
                input.required_gaps.join("；")
            },
        );
    }

    // G3 必填：简介
    add(
        "G3 简介",
        !input.descr.trim().is_empty(),
        if input.descr.trim().is_empty() {
            "简介为空".into()
        } else {
            format!("简介 {} 字节", input.descr.len())
        },
    );

    // G3 必填：简介图片数量（按最终提交值，死图被剪掉后会在这里暴露）
    if let Some(min) = input.rules.require_images {
        let n = image_count(input.descr);
        add(
            "G3 简介图片",
            n >= min,
            if n >= min {
                format!("简介图片 {n} 张 ≥ {min}")
            } else {
                format!("简介图片仅 {n} 张 < 站点要求 {min}（跑图床重传或补截图）")
            },
        );
    }

    // G3 海报 / 截图拆分（站点按独立字段或首图拆分要求时才检查）
    if let Some(min_poster) = input.rules.require_poster {
        add(
            "G3 海报",
            input.poster_count >= min_poster,
            if input.poster_count >= min_poster {
                format!("海报 {} 张 ≥ {min_poster}", input.poster_count)
            } else {
                format!(
                    "海报仅 {} 张 < 站点要求 {min_poster}（补海报）",
                    input.poster_count
                )
            },
        );
    }
    if let Some(min_shots) = input.rules.require_screenshots {
        add(
            "G3 截图",
            input.screenshot_count >= min_shots,
            if input.screenshot_count >= min_shots {
                format!("截图 {} 张 ≥ {min_shots}", input.screenshot_count)
            } else {
                format!(
                    "截图仅 {} 张 < 站点要求 {min_shots}（补截图）",
                    input.screenshot_count
                )
            },
        );
    }

    // G3 简介顺序（配置了才检查；标记按字面匹配）
    if !input.rules.descr_order.is_empty() {
        let (missing, out_of_order) = descr_order_gaps(input.descr, &input.rules.descr_order);
        let ok = missing.is_empty() && out_of_order.is_empty();
        let mut msg = if ok {
            format!("简介顺序符合：{}", input.rules.descr_order.join(" → "))
        } else {
            let mut parts = Vec::new();
            if !missing.is_empty() {
                parts.push(format!("缺标记 {}", missing.join("/")));
            }
            if !out_of_order.is_empty() {
                parts.push(format!("顺序错误 {}", out_of_order.join("/")));
            }
            format!(
                "简介顺序不合规：{}；要求 {}",
                parts.join("；"),
                input.rules.descr_order.join(" → ")
            )
        };
        if !ok {
            msg.push_str("（源简介需重排后再发）");
        }
        add("G3 简介顺序", ok, msg);
    }

    if input.rules.require_mediainfo() {
        add(
            "G3 MediaInfo",
            !input.mediainfo.trim().is_empty(),
            if input.mediainfo.trim().is_empty() {
                "MediaInfo 为空（站点要求必填）".into()
            } else {
                format!("MediaInfo {} 字节", input.mediainfo.len())
            },
        );
    }

    // G3 必填：条目链接（规则可放宽）
    if input.rules.require_links() {
        add(
            "G3 条目链接",
            input.has_links,
            if input.has_links {
                "简介带 IMDb/豆瓣/TMDB 链接".into()
            } else {
                "简介缺 IMDb/豆瓣/TMDB 链接".into()
            },
        );
    }

    // G4 标题长度
    if let Some(max) = input.rules.title_max_len {
        let n = input.name.chars().count();
        add(
            "G4 标题长度",
            n <= max,
            if n <= max {
                format!("标题 {n} 字符 ≤ {max}")
            } else {
                format!("标题 {n} 字符 > 站点上限 {max}")
            },
        );
    }

    // G4 标题禁中文（站点明确要求主标题纯英文）
    if input.rules.title_no_cjk.unwrap_or(false) {
        let bad = has_cjk(input.name);
        add(
            "G4 标题禁中文",
            !bad,
            if bad {
                format!("主标题含汉字：{}（中文名请放副标题）", input.name)
            } else {
                "主标题不含汉字".into()
            },
        );
    }

    // G4 禁发关键词
    if !input.rules.banned_keywords.is_empty() {
        let hay = format!("{} {} {}", input.name, input.subtitle, input.descr).to_lowercase();
        let hit: Vec<&String> = input
            .rules
            .banned_keywords
            .iter()
            .filter(|k| hay.contains(&k.to_lowercase()))
            .collect();
        add(
            "G4 禁发词",
            hit.is_empty(),
            if hit.is_empty() {
                "未命中禁发词".into()
            } else {
                format!(
                    "命中禁发词：{}",
                    hit.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("/")
                )
            },
        );
    }

    // G5 官方预审
    match input.audit_passed {
        Some(true) => add("G5 官方预审", true, "官方预审通过".into()),
        Some(false) => add("G5 官方预审", false, "官方预审未通过（❌ 未清零）".into()),
        None => add("G5 官方预审", true, "站点无官方预审接口（跳过）".into()),
    }

    out
}

/// 门禁汇总：全 Pass 才允许自动发
pub fn all_passed(results: &[GateResult]) -> bool {
    results.iter().all(|r| r.verdict == Verdict::Pass)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input<'a>(rules: &'a SiteRules) -> GateInput<'a> {
        GateInput {
            rules,
            name: "Show.Complete.1080p.HDTV.x264-GRP",
            subtitle: "国产剧 全41集",
            descr: "简介 https://www.imdb.com/title/tt1/",
            mediainfo: "General\nFormat : MPEG-4",
            has_links: true,
            type_label: Some("剧集"),
            core_gaps: &[],
            required_gaps: &[],
            poster_count: 0,
            screenshot_count: 0,
            audit_passed: Some(true),
        }
    }

    #[test]
    fn upload_timeout_defaults_to_120_and_honors_site_override() {
        let mut rules = SiteRules::default();
        assert_eq!(rules.upload_timeout_sec(), 120);
        rules.upload_timeout_sec = Some(300);
        assert_eq!(rules.upload_timeout_sec(), 300);
        rules.upload_timeout_sec = Some(0);
        assert_eq!(rules.upload_timeout_sec(), 1);
    }

    #[test]
    fn clean_input_passes_all_gates() {
        let rules = SiteRules::default();
        let out = evaluate(&base_input(&rules));
        assert!(all_passed(&out), "{out:?}");
        assert!(out.iter().any(|r| r.gate == "G5 官方预审"));
    }

    #[test]
    fn quality_gap_blocks() {
        let rules = SiteRules::default();
        let gaps = vec!["medium_sel 未选".to_string()];
        let mut input = base_input(&rules);
        input.core_gaps = &gaps;
        let out = evaluate(&input);
        assert!(!all_passed(&out));
        let g2 = out.iter().find(|r| r.gate == "G2 质量档").unwrap();
        assert_eq!(g2.verdict, Verdict::Block);
    }

    #[test]
    fn type_gap_and_whitelist_block() {
        let rules = SiteRules {
            allowed_types: vec!["电视剧".into(), "电影".into()],
            ..Default::default()
        };
        let gaps = vec!["type 未选".to_string()];
        let mut input = base_input(&rules);
        input.core_gaps = &gaps;
        input.type_label = Some("综艺");
        let out = evaluate(&input);
        assert!(out
            .iter()
            .any(|r| r.gate == "G1 分类" && r.verdict == Verdict::Block));
        assert!(out
            .iter()
            .any(|r| r.gate == "G1 分类白名单" && r.verdict == Verdict::Block));
    }

    #[test]
    fn required_fields_and_audit_block() {
        let rules = SiteRules::default();
        let mut input = base_input(&rules);
        input.mediainfo = "";
        input.has_links = false;
        input.audit_passed = Some(false);
        let out = evaluate(&input);
        assert!(out
            .iter()
            .any(|r| r.gate == "G3 MediaInfo" && r.verdict == Verdict::Block));
        assert!(out
            .iter()
            .any(|r| r.gate == "G3 条目链接" && r.verdict == Verdict::Block));
        assert!(out
            .iter()
            .any(|r| r.gate == "G5 官方预审" && r.verdict == Verdict::Block));
    }

    #[test]
    fn title_len_and_banned_words_block() {
        let rules = SiteRules {
            title_max_len: Some(10),
            banned_keywords: vec!["枪版".into()],
            ..Default::default()
        };
        let mut input = base_input(&rules);
        input.descr = "这里是枪版资源";
        let out = evaluate(&input);
        assert!(out
            .iter()
            .any(|r| r.gate == "G4 标题长度" && r.verdict == Verdict::Block));
        assert!(out
            .iter()
            .any(|r| r.gate == "G4 禁发词" && r.verdict == Verdict::Block));
    }

    #[test]
    fn required_field_gap_blocks_with_message() {
        let rules = SiteRules {
            required_fields: vec!["region_sel".into(), "style_sel".into()],
            ..Default::default()
        };
        let gaps = vec![
            "必填缺失：region_sel[4]".into(),
            "必填缺失：style_sel[4][]（配置标签未命中：中字）".into(),
        ];
        let mut input = base_input(&rules);
        input.required_gaps = &gaps;
        let out = evaluate(&input);
        let gate = out
            .iter()
            .find(|r| r.gate == "G3 必填字段")
            .expect("应出现必填字段门禁");
        assert_eq!(gate.verdict, Verdict::Block);
        assert!(gate.message.contains("必填缺失"));
        assert!(gate.message.contains("style_sel[4][]"));
        assert!(!all_passed(&out));
    }

    #[test]
    fn required_field_gate_shows_local_gap_when_unconfigured() {
        // 站点没配 requiredFields/requiredTags，但已有本地缺口（如分类异步字段
        // 派生不出值）：门禁仍要把缺口展示出来并拦下，和真实提交前硬拦口径一致。
        let rules = SiteRules::default();
        let gaps = vec!["必填缺失：region_sel[4]".into()];
        let mut input = base_input(&rules);
        input.required_gaps = &gaps;
        let out = evaluate(&input);
        let gate = out
            .iter()
            .find(|r| r.gate == "G3 必填字段")
            .expect("有本地缺口就应出现必填字段门禁");
        assert_eq!(gate.verdict, Verdict::Block);
        assert!(gate.message.contains("region_sel[4]"), "{gate:?}");
        assert!(!all_passed(&out));
    }

    #[test]
    fn required_field_filled_passes() {
        let rules = SiteRules {
            required_fields: vec!["region_sel".into()],
            ..Default::default()
        };
        let input = base_input(&rules);
        let out = evaluate(&input);
        let gate = out
            .iter()
            .find(|r| r.gate == "G3 必填字段")
            .expect("应出现必填字段门禁");
        assert_eq!(gate.verdict, Verdict::Pass);
    }

    #[test]
    fn rules_can_relax_optional_fields() {
        let rules = SiteRules {
            require_links: Some(false),
            require_mediainfo: Some(false),
            ..Default::default()
        };
        let mut input = base_input(&rules);
        input.mediainfo = "";
        input.has_links = false;
        let out = evaluate(&input);
        assert!(all_passed(&out), "{out:?}");
    }

    #[test]
    fn title_no_cjk_blocks_chinese_main_title() {
        let rules = SiteRules {
            title_no_cjk: Some(true),
            ..Default::default()
        };
        let mut input = base_input(&rules);
        input.name = "星球大战9：天行者崛起 Star.Wars.Episode.IX.2019.1080p-GRP";
        let out = evaluate(&input);
        let gate = out
            .iter()
            .find(|r| r.gate == "G4 标题禁中文")
            .expect("应出现标题禁中文门禁");
        assert_eq!(gate.verdict, Verdict::Block);
        assert!(gate.message.contains("主标题含汉字"));
    }

    #[test]
    fn title_no_cjk_allows_english_only() {
        let rules = SiteRules {
            title_no_cjk: Some(true),
            ..Default::default()
        };
        let input = base_input(&rules);
        let out = evaluate(&input);
        assert!(all_passed(&out), "{out:?}");
    }

    #[test]
    fn image_count_handles_bbcode_and_html() {
        assert_eq!(image_count("无图"), 0);
        assert_eq!(image_count("[img]https://a/1.jpg[/img]"), 1);
        assert_eq!(
            image_count("[img=200x100]https://a/1.jpg[/img]<img src=\"x\">"),
            2
        );
        assert_eq!(image_count("[img]x[/img]\n[IMG]y[/IMG]"), 2);
        assert_eq!(image_count("[img=]x[/img]"), 1);
        assert_eq!(image_count("[imgs]x[/imgs]"), 0);
    }

    #[test]
    fn require_images_blocks_too_few() {
        let rules = SiteRules {
            require_images: Some(3),
            ..Default::default()
        };
        let mut input = base_input(&rules);
        input.descr = "[img]https://a/1.jpg[/img]\n[img]https://a/2.jpg[/img]";
        let out = evaluate(&input);
        let gate = out
            .iter()
            .find(|r| r.gate == "G3 简介图片")
            .expect("应出现简介图片门禁");
        assert_eq!(gate.verdict, Verdict::Block);
        assert!(gate.message.contains("仅 2 张"));
    }

    #[test]
    fn descr_order_blocks_missing_and_out_of_order() {
        let rules = SiteRules {
            descr_order: vec![
                "【转载出处】".into(),
                "【制作说明】".into(),
                "【海报】".into(),
                "【简介】".into(),
                "【info】".into(),
                "【截图】".into(),
            ],
            ..Default::default()
        };
        let mut input = base_input(&rules);
        input.descr = "【简介】正文\n【海报】[img]x[/img]\n【截图】[img]y[/img]";
        let out = evaluate(&input);
        let gate = out
            .iter()
            .find(|r| r.gate == "G3 简介顺序")
            .expect("应出现简介顺序门禁");
        assert_eq!(gate.verdict, Verdict::Block);
        assert!(gate.message.contains("缺标记"));
        assert!(gate.message.contains("顺序错误"));
    }

    #[test]
    fn descr_order_passes_when_sections_in_order() {
        let rules = SiteRules {
            descr_order: vec!["【转载出处】".into(), "【海报】".into(), "【截图】".into()],
            ..Default::default()
        };
        let mut input = base_input(&rules);
        input.descr = "【转载出处】TLF\n【海报】[img]x[/img]\n【截图】[img]y[/img]";
        let out = evaluate(&input);
        assert!(all_passed(&out), "{out:?}");
    }

    #[test]
    fn image_floor_detail_covers_new_sites() {
        let agsvpt = image_floor_detail("agsvpt").unwrap();
        assert_eq!(agsvpt.min_total, 2);
        assert_eq!(agsvpt.min_poster, 1);
        assert_eq!(agsvpt.min_screenshots, 1);
        let tjupt = image_floor_detail("tjupt").unwrap();
        assert_eq!(tjupt.min_total, 1);
        assert_eq!(tjupt.min_poster, 0);
        assert_eq!(tjupt.min_screenshots, 0);
        let ptlgs = image_floor_detail("ptlgs").unwrap();
        assert_eq!(ptlgs.min_total, 4);
        assert_eq!(ptlgs.min_poster, 1);
        assert_eq!(ptlgs.min_screenshots, 3);
        // 原有总图数站点不拆海报/截图（回归：不能把 3 张按 1 海报 + 2 截图误拦）
        let cangbao = image_floor_detail("cangbao").unwrap();
        assert_eq!(
            (
                cangbao.min_total,
                cangbao.min_poster,
                cangbao.min_screenshots
            ),
            (3, 0, 0)
        );
        let panda = image_floor_detail("panda").unwrap();
        assert_eq!(
            (panda.min_total, panda.min_poster, panda.min_screenshots),
            (4, 0, 0)
        );
        assert!(image_floor_detail("unknown").is_none());
    }

    #[test]
    fn required_tag_52_gate_blocks_when_missing_then_passes() {
        // 配置声明上传必须勾 ⛵方舟（value 52；13 是杜比、51 是合并，不要混）
        let rules = SiteRules {
            required_tags: vec![RequiredTag {
                field: "tags[4][]".into(),
                value: Some("52".into()),
                label: Some("方舟".into()),
            }],
            ..Default::default()
        };
        // 红：配置要求的 52 标签没勾上 → 必填字段门禁拦下，并点名是哪个标签
        let gaps = vec!["必填缺失：tags[4][]（必需标签「方舟」未勾选）".into()];
        let mut input = base_input(&rules);
        input.required_gaps = &gaps;
        let out = evaluate(&input);
        let gate = out
            .iter()
            .find(|r| r.gate == "G3 必填字段")
            .expect("配了 requiredTags 就要出现必填字段门禁");
        assert_eq!(gate.verdict, Verdict::Block);
        assert!(gate.message.contains("方舟"));
        assert!(!all_passed(&out));
        // 绿：标签已勾上、无缺口 → 通过
        let input2 = base_input(&rules);
        let out2 = evaluate(&input2);
        assert!(all_passed(&out2), "{out2:?}");
    }

    #[test]
    fn poster_screenshot_split_gate_blocks_and_passes() {
        let rules = SiteRules {
            require_poster: Some(1),
            require_screenshots: Some(1),
            ..Default::default()
        };
        // 红：没海报也没截图
        let mut input = base_input(&rules);
        input.poster_count = 0;
        input.screenshot_count = 0;
        let out = evaluate(&input);
        assert!(out
            .iter()
            .any(|r| r.gate == "G3 海报" && r.verdict == Verdict::Block));
        assert!(out
            .iter()
            .any(|r| r.gate == "G3 截图" && r.verdict == Verdict::Block));
        // 红：有海报、没截图
        input.poster_count = 1;
        let out = evaluate(&input);
        assert!(out
            .iter()
            .any(|r| r.gate == "G3 截图" && r.verdict == Verdict::Block));
        // 绿：海报 1、截图 1
        input.screenshot_count = 1;
        let out = evaluate(&input);
        assert!(all_passed(&out), "{out:?}");
    }

    #[test]
    fn site_rules_deserialize_allowed_types() {
        // 配置里的 camelCase 字段必须真的落到结构体，否则 iloli 只收动画的白名单是空转。
        let rules: SiteRules =
            serde_json::from_str(r#"{"allowedTypes":["动画"],"selectPicks":{"type":"405"}}"#)
                .unwrap();
        assert_eq!(rules.allowed_types, vec!["动画".to_string()]);
        assert_eq!(
            rules.select_picks.get("type").map(String::as_str),
            Some("405")
        );
    }
}
