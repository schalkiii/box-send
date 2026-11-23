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

/// 站点规范（`assist.reseedRules.<siteId>`；缺省全保守）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteRules {
    /// 标题最大长度（字符）；超过即 Block
    pub title_max_len: Option<usize>,
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
    /// 必填字段名前缀（如 `["region_sel","style_sel"]`）：任一前缀下没有已选值
    /// 即判核心缺口并阻断发布；未配置的站点行为不变。
    #[serde(default)]
    pub required_fields: Vec<String>,
    /// 提交前剪掉简介里无法访问的图片（站点官方预审会逐图校验）
    pub prune_dead_images: Option<bool>,
    /// 简介格式：缺省 `bbcode` = 提交前把 HTML 转 BBCode（NexusPHP 的简介框是
    /// BBCode 编辑器，原样提交 HTML 会被转义成字面 `<fieldset>`）；显式 `html`
    /// 才保持 HTML 原样（仅用于确认支持 HTML 的站）。
    pub descr_format: Option<String>,
    /// 每日自动发种上限（缺省 2）
    pub daily_limit: Option<u32>,
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
    /// 简介是否按 BBCode 提交：缺省是；只有显式 `html` 才保持 HTML。
    pub fn descr_is_bbcode(&self) -> bool {
        self.descr_format
            .as_deref()
            .is_none_or(|f| !f.eq_ignore_ascii_case("html"))
    }
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

    // G3 必填字段（规则配置的单选/复选组；未配置 requiredFields 不参与判定）
    if !input.rules.required_fields.is_empty() {
        add(
            "G3 必填字段",
            input.required_gaps.is_empty(),
            if input.required_gaps.is_empty() {
                "必填字段均已选中".into()
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

    // G3 必填：MediaInfo（规则可放宽）
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
            audit_passed: Some(true),
        }
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
    fn required_field_gate_absent_when_unconfigured() {
        // 旧配置不配 requiredFields：即使外部塞了缺口，门禁不出现、行为不变
        let rules = SiteRules::default();
        let gaps = vec!["必填缺失：region_sel[4]".into()];
        let mut input = base_input(&rules);
        input.required_gaps = &gaps;
        let out = evaluate(&input);
        assert!(!out.iter().any(|r| r.gate == "G3 必填字段"));
        assert!(all_passed(&out), "{out:?}");
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
}
