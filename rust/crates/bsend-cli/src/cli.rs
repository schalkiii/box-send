//! 命令行参数定义。处理逻辑仍在 `main.rs`，这里只放 clap 结构和解析测试。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "bsend",
    version,
    about = "辅种引擎：内容索引 → 源种定位 → 辅种/转种（box-send Rust 版）"
)]
pub(crate) struct Cli {
    /// boxsend.json 路径（与 GUI 版共用同一份配置）
    #[arg(long, default_value = "boxsend.json")]
    pub(crate) config: PathBuf,
    /// 本轮从 CookieCloud 拉取 cookie（覆盖 cookies.json 里同 host 的条目，不写盘）
    #[arg(long, global = true)]
    pub(crate) cookiecloud: bool,
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Cmd {
    /// 扫描下载器，建立/刷新内容索引（不动站点）
    Scan,
    /// 预演一轮：检索照跑，推送只出「可辅」结论，不落盘不真加
    Plan {
        /// 最多处理多少格
        #[arg(long)]
        limit: Option<usize>,
        /// 只跑这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// 只处理这段内容（contentKey 前缀或名称包含；调试/单条重试用）
        #[arg(long)]
        only: Option<String>,
    },
    /// 跑一整轮辅种：辅种真推下载器，转种出草稿清单
    Run {
        /// 最多处理多少格
        #[arg(long)]
        limit: Option<usize>,
        /// 只跑这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// 不真加下载器（与 plan 的区别：run --dry-run 会把结论落盘）
        #[arg(long)]
        dry_run: bool,
        /// 只处理这段内容（contentKey 前缀或名称包含；调试/单条重试用）
        #[arg(long)]
        only: Option<String>,
    },
    /// 只做基线：把「已在做种」的覆盖记录标好并落盘，零站点请求（先看清家底）
    Prime,
    /// 看 cookie 现状：逐站条目数、最早过期、有没有鉴权字段（不打印 cookie 值）
    Cookies {
        /// 数据来源：json（读 cookies.json）或 cookiecloud（现场拉取解密）
        #[arg(long, default_value = "json")]
        from: String,
        /// 把结果写回 cookies.json（GUI 读同一份；先备份成 cookies.json.bak）
        #[arg(long)]
        write: bool,
        /// 列出全部域名（默认只看参与辅种的站点）
        #[arg(long)]
        all: bool,
    },
    /// 看索引现状：内容数、覆盖分布、转种草稿、待找源
    Status,
    /// 按站点汇总已发种数量与最后发种时间（只读 auto-audit / reseed-log）
    Stats {
        /// 输出 camelCase JSON（给脚本/GUI 吃）
        #[arg(long)]
        json: bool,
    },
    /// 转种草稿的审阅与提交（默认只审阅不提交）
    Reseed {
        /// 转第 N 条草稿（1 起编号；不带此参数只列草稿）
        #[arg(long, conflicts_with = "only")]
        draft: Option<usize>,
        /// 只选名称或 content key 包含这段文字的草稿（单条调试用，与 --draft 互斥）
        #[arg(long)]
        only: Option<String>,
        /// 只选这个目标站（如 iloli）；同一内容在多个站有缺口时避免选错站
        #[arg(long)]
        site: Option<String>,
        /// 真提交（预检 → 上传 → 发后自检 → 推种校验）；不带则只审阅
        #[arg(long)]
        yes: bool,
    },
    /// 覆盖报告：逐站覆盖率、未覆盖最多清单、草稿与待找源（可导出 JSON）
    Report {
        /// 未覆盖清单显示多少条
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// 导出 JSON（camelCase，原子写；给 GUI / daemon 吃）
        #[arg(long)]
        json: Option<PathBuf>,
        /// 只看这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
    },
    /// 发种表格演练（只审不发）：逐站拉 upload.php 构造字段核对完备性，绝不 POST
    Formcheck {
        /// 只查这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
    },
    /// 全站发布检查演练（只读）：构造上传字段 + 官方预审 + 发布检查矩阵，绝不上传种子
    Gatecheck {
        /// 只演练这些站（逗号分隔站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },
    /// 发种审计周报（只读）：按天 × 站点聚合 auto-audit.jsonl，不写覆盖记录不发请求
    Audit {
        /// 统计最近多少天（含今天，北京时间日界）
        #[arg(long, default_value_t = 7)]
        days: usize,
        /// 只看这个目标站 id
        #[arg(long)]
        site: Option<String>,
        /// 输出 camelCase JSON（到 stdout，不写文件）
        #[arg(long)]
        json: bool,
    },

    /// 自动发种：草稿 → 审阅 → 发布检查 → 发布（白名单 + 日限额 + 审计）；--dry-run 只跑发布检查
    Autopost {
        /// 只跑发布检查与审阅，不发布
        #[arg(long)]
        dry_run: bool,
        /// 每轮最多发布多少条（--loop 时按轮计，每轮各自受此限额约束）
        #[arg(long)]
        limit: Option<usize>,
        /// 只处理这些目标站的草稿（逗号分隔站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// 关闭草稿对账（下载器已挂目标站完成种子的草稿覆盖记录自动改写 Seeding）；只关对账
        #[arg(long)]
        no_reconcile: bool,
        /// 串行循环跑多轮：每轮都是一次完整自动发种（失败冷却/日限额/白名单/锁复用单轮逻辑）
        #[arg(long)]
        loop_mode: bool,
        /// 循环模式轮间间隔秒数（默认 1800）
        #[arg(long, default_value_t = 1800)]
        interval_secs: u64,
    },

    /// 把站点上已发布的种子下回来挂进 qB（不重新上传、不占发布额度）
    #[command(alias = "reattach")]
    Pull {
        /// 目标站 id（boxsend.json 的 sourceSites[].id）
        #[arg(long)]
        site: String,
        /// 站上详情页 URL（download.php 也可，会按 referer 解析）
        #[arg(long)]
        url: String,
        /// 内容 key（m:…）；省略则按目标种 pieces 在索引里自动找
        #[arg(long)]
        key: Option<String>,
    },

    /// 回访待跟进的种子：再看一眼“等站点审核 / 审核没过 / 站上已找不到”的详情页，审完了就把站点种子挂回 qB（不上传、不发帖）
    #[command(alias = "reviewcheck")]
    Followup {
        /// 只看和报告：不改覆盖记录，不往下载器里加种子（照样会打开详情页、下载站点种子）
        #[arg(long)]
        dry_run: bool,
        /// 连我们自己发的、已经在做种 / 校验中的条目也回访一遍（找出还没记下来的“未审 / 审核不通过”）
        #[arg(long)]
        all: bool,
        /// 只回访这些站（逗号分隔的站点 id）
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// 最多回访多少条
        #[arg(long)]
        limit: Option<usize>,
        /// 输出 JSON（给脚本/GUI 吃）
        #[arg(long)]
        json: bool,
    },

    /// 站点档案汇总：配置开关 + 最近一次实测结论 + pieces 支持 + cookie 概况
    Sites {
        /// 输出 JSON（给脚本/GUI 吃）
        #[arg(long)]
        json: bool,
        /// 输出 Markdown 表格（写文档用）
        #[arg(long)]
        md: bool,
    },

    /// 落盘核对（只读）：抽若干内容，用下载器实时数据核对「推种时会用哪个 savepath」
    Layoutcheck {
        /// 最多看多少段内容
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// 只看这段内容（contentKey 前缀或名称包含）
        #[arg(long)]
        only: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_uses_default_config_and_no_cookiecloud() {
        let cli = Cli::try_parse_from(["bsend", "status"]).unwrap();
        assert_eq!(cli.config, PathBuf::from("boxsend.json"));
        assert!(!cli.cookiecloud);
        assert!(matches!(cli.cmd, Cmd::Status));
    }

    #[test]
    fn reseed_accepts_site_draft_and_yes() {
        let cli = Cli::try_parse_from([
            "bsend", "reseed", "--draft", "7", "--site", "iloli", "--yes",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Reseed {
                draft,
                site,
                yes,
                only,
            } => {
                assert_eq!(draft, Some(7));
                assert_eq!(site.as_deref(), Some("iloli"));
                assert!(yes);
                assert!(only.is_none());
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn reseed_rejects_draft_with_only() {
        let err = Cli::try_parse_from(["bsend", "reseed", "--draft", "1", "--only", "Poirot"])
            .unwrap_err();
        assert!(err.to_string().contains("cannot be used with"), "{err}");
    }

    #[test]
    fn pull_keeps_site_and_url() {
        let cli = Cli::try_parse_from([
            "bsend",
            "pull",
            "--site",
            "pthome",
            "--url",
            "https://www.pthome.net/details.php?id=285040",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Pull { site, url, key } => {
                assert_eq!(site, "pthome");
                assert!(url.contains("285040"));
                assert!(key.is_none());
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
