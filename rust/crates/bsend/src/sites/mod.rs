//! 站点目录与框架路由。148 站内置表从 Swift 版 `Sites/SiteRegistry.swift` 的
//! `prioritySites` 逐条移植（id/名称/域名/框架/默认启用）——它是「按 URL 认站」的底册，
//! 界面上只会显示用户配置里添加过的站，这里给的是识别与默认参数。

pub mod nexusphp;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Framework {
    NexusPHP,
    Unit3D,
    Gazelle,
    GazelleJSONAPI,
    Luminance,
    AvistazNetwork,
    Blu,
    TNode,
    Haidan,
    YemaPT,
    Discuz,
    Cola,
    Xbtit,
    Custom,
}

impl Framework {
    /// boxsend.json 里的 framework 字段值（与 Swift `SiteFramework.rawValue` 一致）
    pub fn parse(s: &str) -> Framework {
        match s.to_lowercase().as_str() {
            "nexusphp" => Framework::NexusPHP,
            "unit3d" => Framework::Unit3D,
            "gazelle" => Framework::Gazelle,
            "gazellejsonapi" => Framework::GazelleJSONAPI,
            "luminance" => Framework::Luminance,
            "avistaznetwork" => Framework::AvistazNetwork,
            "blu" => Framework::Blu,
            "tnode" => Framework::TNode,
            "haidan" => Framework::Haidan,
            "yemapt" => Framework::YemaPT,
            "discuz" => Framework::Discuz,
            "cola" => Framework::Cola,
            "xbtit" => Framework::Xbtit,
            _ => Framework::Custom,
        }
    }

    /// v1 检索臂只认 NexusPHP 通用端点（`torrents.php?search=`）。
    /// 其余框架的检索在 Swift 版里各有实现，Rust 版按流量逐步搬（框架覆盖现状见 `doc/SITES.md`）。
    pub fn search_capable(&self) -> bool {
        matches!(self, Framework::NexusPHP)
    }
}

pub struct SiteDef {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub framework: Framework,
    pub default_enabled: bool,
}

/// 站点 announce 用的额外域名（页面主域之外）。
///
/// 实机 42,190 条种子的 tracker host 统计出来的真实差异：
/// - luckpt：页面在 `pt.luckpt.de`，announce 在 `tracker.luckpt.de`（565 条种子）；
/// - chdbits：页面在 `ptchdbits.co`，announce 在 `chdbits.xyz`（136 条种子）；
/// - baozi：页面在 `p.t-baozi.cc`，announce 在 `tracker.t-baozi.cc`（607 条种子）；
/// - novahd：页面在 `pt.novahd.top`，announce 在 `tracker.novahd.top`（641 条种子）；
/// - xingyunge：页面在 `pt.xingyungept.org`，announce 在 `tracker.xingyungept.org`（717 条种子）；
/// - ptlgs：页面在 `ptlgs.org`，announce 在 `ptl.gs`（594 条种子）；
/// - pthome：页面在 `www.pthome.net`，announce 在 `t.pthome.org`（411 条种子）；
/// - agsvpt：页面在 `agsvpt.com`，announce 在 `tracker.agsvpt.cn`（806 条种子）；
/// - cspt：页面在 `cspt.top`，站上种子的 announce 在 `tracker.cspt.cc`。
/// - tey：页面在 `pt.tey.cc`，qB 里 5 条种子全走 `tracker.tey.cc`；
///   不补别名会误判成「qB 覆盖 0」，连 passkey 都抄不到。
///
/// 不补这些别名，「已在做种」预筛会把它们当成未覆盖去逐站检索（hdsky 那种 60 秒/次的站，
/// 代价极高）。配置里 `overrides.announceHosts` 可以再追加（GUI 写的配置没有这一项也能跑）。
pub fn extra_announce_hosts(site_id: &str) -> &'static [&'static str] {
    match site_id {
        "luckpt" => &["tracker.luckpt.de"],
        "chdbits" => &["chdbits.xyz", "chdbits.co"],
        // 页面在 www.hdkyl.in，announce 在 tracker.hdkyl.in；不补别名 qB 真值会漏判。
        "qilin" => &["tracker.hdkyl.in"],
        "baozi" => &["tracker.t-baozi.cc"],
        "novahd" => &["tracker.novahd.top"],
        "xingyunge" => &["tracker.xingyungept.org"],
        "ptlgs" => &["ptl.gs"],
        "pthome" => &["t.pthome.org"],
        "agsvpt" => &["tracker.agsvpt.cn"],
        "cspt" => &["tracker.cspt.cc"],
        "tey" => &["tracker.tey.cc"],
        _ => &[],
    }
}

/// 该站发布后会不会改写种子元信息，导致目标种 hash 与上传构造的 hash 不同。
///
/// 只认实机验证过的站：1ptba 的 takeupload.php 会把 `info.source` 改成
/// `[1ptba.com] 1PTBA.COM`；cspt、dubhe、ptsbao、crabpt、hdbao、tey、oshen 实测也出现 hash
/// 差异，pieces 和 `info.name` 未变，但具体被改写的字段没有逐字节核对。挂回时按分片+标题校验放行，
/// 仍拒绝分片不一致的种。不能把它推广成「hash 不一致就放行」，否则会把别的版本塞进 qB。
pub fn allows_target_hash_rewrite(site_id: &str) -> bool {
    matches!(
        site_id,
        "ptba" | "cspt" | "dubhe" | "ptsbao" | "crabpt" | "hdbao" | "tey" | "oshen"
    )
}

/// tracker host 是否属于这个站点：相等，或 tracker 在站点域名的子域上。
/// NexusPHP 站的 announce 常在 `tracker.xxx.com` 而页面在 `xxx.com`——
/// 精确相等会把这类站全部漏判（已在做种预筛、源站优先排序都靠它）。
/// `www.` 只是页面前缀，不属于站点身份：`www.tjupt.org` 与 `tjupt.org`、
/// `tracker-public.tjupt.org` 应视为同一站，先剥掉再比。
pub fn host_matches(tracker_host: &str, site_host: &str) -> bool {
    if tracker_host.is_empty() || site_host.is_empty() {
        return false;
    }
    let tracker_host = tracker_host.strip_prefix("www.").unwrap_or(tracker_host);
    let site_host = site_host.strip_prefix("www.").unwrap_or(site_host);
    tracker_host == site_host || tracker_host.ends_with(&format!(".{site_host}"))
}

/// 内置站点表（顺序、条目与 Swift `SiteRegistry.prioritySites` 一致）
static SITE_TABLE: &[SiteDef] = &[
    // --- 9 个优先站（Swift 端默认启用） ---
    SiteDef {
        id: "luckpt",
        name: "幸运",
        url: "https://pt.luckpt.de/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    SiteDef {
        id: "hdsky",
        name: "天空",
        url: "https://hdsky.me/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    SiteDef {
        id: "chdbits",
        name: "彩虹岛",
        url: "https://ptchdbits.co/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    SiteDef {
        id: "hdhome",
        name: "家园",
        url: "https://hdhome.org/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    SiteDef {
        id: "cmct",
        name: "春天",
        url: "https://springsunday.net/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    SiteDef {
        id: "audiences",
        name: "观众",
        url: "https://audiences.me/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    SiteDef {
        id: "ttg",
        name: "套",
        url: "https://totheglory.im/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    SiteDef {
        id: "pter",
        name: "猫",
        url: "https://pterclub.net/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    SiteDef {
        id: "hhanclub",
        name: "憨憨",
        url: "https://hhanclub.net/",
        framework: Framework::NexusPHP,
        default_enabled: true,
    },
    // --- Blu 家族（Layout UI） ---
    SiteDef {
        id: "blutopia",
        name: "Blutopia",
        url: "https://blutopia.cc/",
        framework: Framework::Blu,
        default_enabled: false,
    },
    SiteDef {
        id: "monika",
        name: "莫妮卡",
        url: "https://monikadesign.uk/",
        framework: Framework::Blu,
        default_enabled: false,
    },
    // --- 经典 Gazelle / xbtit ---
    SiteDef {
        id: "hdspace",
        name: "HD-Space",
        url: "https://hd-space.org/",
        framework: Framework::Gazelle,
        default_enabled: false,
    },
    SiteDef {
        id: "opencd",
        name: "皇后",
        url: "https://open.cd/",
        framework: Framework::Gazelle,
        default_enabled: false,
    },
    SiteDef {
        id: "iptorrents",
        name: "IPT",
        url: "https://iptorrents.com/",
        framework: Framework::Gazelle,
        default_enabled: false,
    },
    // --- 特殊 NexusPHP（自定义字段/分类） ---
    SiteDef {
        id: "byr",
        name: "北邮",
        url: "https://byr.pt/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "shadow",
        name: "影",
        url: "https://star-space.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    // --- 通用中文 NexusPHP（动态分类解析，免逐站配置） ---
    SiteDef {
        id: "city13",
        name: "13City",
        url: "https://13city.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptba",
        name: "1PT",
        url: "https://1ptba.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "movie52",
        name: "52MOVIE",
        url: "https://www.52movie.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "pt52",
        name: "52PT",
        url: "https://52pt.site/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "agsvpt",
        name: "末日",
        url: "https://www.agsvpt.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "railgun",
        name: "Railgun",
        url: "https://bilibili.download/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "cangbao",
        name: "藏宝阁",
        url: "https://cangbao.ge/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "carpt",
        name: "车站",
        url: "https://carpt.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "crabpt",
        name: "蟹黄堡",
        url: "https://crabpt.vip/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "cspt",
        name: "财神",
        url: "https://cspt.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "cyanbug",
        name: "大青虫",
        url: "https://cyanbug.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "discfan",
        name: "蝶粉",
        url: "https://discfan.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "dragonhd",
        name: "龙之家",
        url: "https://www.dragonhd.xyz/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "dubhe",
        name: "天枢",
        url: "https://dubhe.site/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "march",
        name: "三月",
        url: "https://duckboobee.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "tccf",
        name: "TCCF",
        url: "https://et8.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ggpt",
        name: "GGPT",
        url: "https://www.gamegamept.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hdarea",
        name: "高清视界",
        url: "https://hdarea.club/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hdbao",
        name: "海德堡",
        url: "https://hdbao.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hddolby",
        name: "杜比",
        url: "https://www.uhddolby.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hdfans",
        name: "红豆饭",
        url: "http://hdfans.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "qilin",
        name: "麒麟",
        url: "https://www.hdkyl.in/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hdtime",
        name: "时光",
        url: "https://hdtime.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hdvideo",
        name: "HDVideo",
        url: "https://hdvideo.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hitpt",
        name: "百川",
        url: "https://www.hitpt.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "haitang",
        name: "海棠",
        url: "https://www.htpt.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hudbt",
        name: "蝴蝶",
        url: "https://hudbt.hust.edu.cn/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "haoxue",
        name: "好学",
        url: "https://www.hxpt.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ziran",
        name: "自然",
        url: "https://naturept.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "kufei",
        name: "库非",
        url: "https://kufei.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "lajidui",
        name: "垃圾堆",
        url: "https://pt.lajidui.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "lemonhd",
        name: "柠檬不甜",
        url: "https://lemonhd.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "longpt",
        name: "龙",
        url: "https://longpt.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "iloli",
        name: "爱萝莉",
        url: "https://mua.xloli.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "njtupt",
        name: "蒲园",
        url: "https://njtupt.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "okpt",
        name: "OK",
        url: "https://www.okpt.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "oshen",
        name: "奥申",
        url: "http://www.oshen.win/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "baozi",
        name: "包子",
        url: "https://p.t-baozi.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "panda",
        name: "熊猫",
        url: "https://pandapt.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "piggo",
        name: "猪猪",
        url: "https://piggo.me/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "freefarm",
        name: "农场",
        url: "https://pt.0ff.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "aling",
        name: "爱玲",
        url: "https://pt.aling.de/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "btschool",
        name: "学校",
        url: "https://pt.btschool.club/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "tlf",
        name: "吐鲁番",
        url: "https://pt.eastgame.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "gtk",
        name: "GTK",
        url: "https://pt.gtkpw.xyz/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hdclone",
        name: "独自",
        url: "https://pt.hdclone.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "itzmx",
        name: "Itz",
        url: "https://pt.itzmx.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "muxuege",
        name: "慕雪阁",
        url: "https://pt.muxuege.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "novahd",
        name: "nova",
        url: "https://pt.novahd.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "soulvoice",
        name: "聆音",
        url: "https://pt.soulvoice.club/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hdu",
        name: "好多油",
        url: "https://pt.upxin.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "xingyunge",
        name: "星陨阁",
        url: "https://pt.xingyungept.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "yinghua",
        name: "樱花",
        url: "http://pt.ying.us.kg/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptcafe",
        name: "咖啡",
        url: "https://ptcafe.club/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptfans",
        name: "PTFans",
        url: "https://ptfans.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "pthome",
        name: "铂金家",
        url: "https://www.pthome.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptlgs",
        name: "劳改所",
        url: "https://ptlgs.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptsbao",
        name: "烧包",
        url: "https://ptsbao.club/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptskit",
        name: "拾刻",
        url: "https://www.ptskit.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptt",
        name: "时间",
        url: "https://www.pttime.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptzone",
        name: "葡萄汁",
        url: "https://ptzone.xyz/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "qingwa",
        name: "青蛙",
        url: "https://qingwapt.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "sbpt",
        name: "SBPT",
        url: "https://sbpt.link/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "shuioudao",
        name: "下水道",
        url: "https://sewerpt.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "tangpt",
        name: "躺平",
        url: "https://www.tangpt.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "tjupt",
        name: "北洋园",
        url: "https://www.tjupt.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ubits",
        name: "优堡",
        url: "https://ubits.club/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ultrahd",
        name: "UltraHD",
        url: "https://ultrahd.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "wtsakura",
        name: "冬樱",
        url: "https://wintersakura.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "xingtan",
        name: "杏坛",
        url: "https://xingtan.one/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "zmpt",
        name: "织梦",
        url: "https://zmpt.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "u2",
        name: "幼儿圈",
        url: "http://u2.dmhy.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    // --- TNode / Haidan / YemaPT ---
    SiteDef {
        id: "zhuque",
        name: "朱雀",
        url: "https://zhuque.in/",
        framework: Framework::TNode,
        default_enabled: false,
    },
    SiteDef {
        id: "haidan",
        name: "海胆",
        url: "https://www.haidan.cc/",
        framework: Framework::Haidan,
        default_enabled: false,
    },
    SiteDef {
        id: "yemapt",
        name: "野马",
        url: "https://www.yemapt.org/",
        framework: Framework::YemaPT,
        default_enabled: false,
    },
    // --- 中文 NexusPHP（savept.icu 2026-09-30 批次） ---
    SiteDef {
        id: "azusa",
        name: "梓喵",
        url: "https://azusa.wiki/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "dicmusic",
        name: "海豚",
        url: "https://dicmusic.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "dstudio",
        name: "Depth Studio",
        url: "https://dstudio.me/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "kamept",
        name: "龟站",
        url: "https://kamept.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "nanyang",
        name: "南洋",
        url: "https://nanyangpt.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "kelu",
        name: "Kelu",
        url: "https://our.kelu.one/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "playlet",
        name: "PlayLet",
        url: "https://playlet.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "daxiangjiao",
        name: "大香蕉",
        url: "https://pt.daxiangjiao.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "keepfrds",
        name: "朋友",
        url: "https://pt.keepfrds.com/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "mypt",
        name: "我的PT",
        url: "https://pt.mypt.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "tey",
        name: "太乙",
        url: "https://pt.tey.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "tu88",
        name: "TU88",
        url: "https://pt.tu88.men/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "vclib",
        name: "VC-Lib",
        url: "https://pt.vclib.online/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "ptlao",
        name: "忘年桥",
        url: "https://ptlao.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "rousi",
        name: "肉丝",
        url: "https://rousi.pro/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "yzyy",
        name: "YZYY",
        url: "https://www.yzyy.org/",
        framework: Framework::Discuz,
        default_enabled: false,
    },
    SiteDef {
        id: "siqi",
        name: "思齐",
        url: "https://si-qi.xyz/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "sunny",
        name: "阳光",
        url: "https://sunnypt.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "momentpt",
        name: "瞬间",
        url: "https://www.momentpt.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "musopia",
        name: "音乐乌托邦",
        url: "https://www.musopia.vip/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "nicept",
        name: "老师",
        url: "https://www.nicept.net/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "tokyo",
        name: "Tokyo",
        url: "https://www.tokyo-manga.top/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "xdy",
        name: "修道院",
        url: "https://xdypt.vip/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "xingwan",
        name: "星湾",
        url: "https://xingwan.cc/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "hdcity",
        name: "城市",
        url: "https://hdcity.city/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "fenghuang",
        name: "凤凰",
        url: "https://pt.521.best/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    SiteDef {
        id: "cola",
        name: "可乐",
        url: "https://www.colapt.com/",
        framework: Framework::Cola,
        default_enabled: false,
    },
    SiteDef {
        id: "generationfree",
        name: "Generation-Free",
        url: "https://generation-free.org/",
        framework: Framework::NexusPHP,
        default_enabled: false,
    },
    // --- Unit3D 家族 ---
    SiteDef {
        id: "mteam",
        name: "馒头",
        url: "https://kp.m-team.cc/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "oldtoons",
        name: "Oldtoons",
        url: "https://oldtoons.world/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "milkie",
        name: "奶昔",
        url: "https://milkie.cc/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "ptneko",
        name: "超科学PT喵",
        url: "https://ptneko.com/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "anthelion",
        name: "Anthelion",
        url: "https://anthelion.me/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "brokenstones",
        name: "BrokenStones",
        url: "https://brokenstones.is/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "exoticaz",
        name: "ExoticaZ",
        url: "https://exoticaz.to/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "filelist",
        name: "FileList",
        url: "https://filelist.io/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "happyfappy",
        name: "HappyFappy",
        url: "https://happyfappy.net/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "jpopsuki",
        name: "Jpopsuki",
        url: "https://jpopsuki.eu/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "nebulance",
        name: "Nebulance",
        url: "https://nebulance.io/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "orpheus",
        name: "Orpheus",
        url: "https://orpheus.network/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    SiteDef {
        id: "empornium",
        name: "峨眉派",
        url: "https://www.empornium.sx/",
        framework: Framework::Unit3D,
        default_enabled: false,
    },
    // --- 经典 Gazelle 家族 ---
    SiteDef {
        id: "alpharatio",
        name: "AlphaRatio",
        url: "https://alpharatio.cc/",
        framework: Framework::Gazelle,
        default_enabled: false,
    },
    SiteDef {
        id: "animez",
        name: "AnimeZ",
        url: "https://animez.to/",
        framework: Framework::Gazelle,
        default_enabled: false,
    },
    SiteDef {
        id: "sportscult",
        name: "SportsCult",
        url: "https://sportscult.org/",
        framework: Framework::Gazelle,
        default_enabled: false,
    },
    // --- xbtit 家族 ---
    SiteDef {
        id: "beyondhd",
        name: "BeyondHD",
        url: "https://beyond-hd.me/",
        framework: Framework::Xbtit,
        default_enabled: false,
    },
    SiteDef {
        id: "clearjav",
        name: "ClearJAV",
        url: "https://clearjav.com/",
        framework: Framework::Xbtit,
        default_enabled: false,
    },
    SiteDef {
        id: "fappaizuri",
        name: "Fappaizuri",
        url: "https://fappaizuri.me/",
        framework: Framework::Xbtit,
        default_enabled: false,
    },
    SiteDef {
        id: "huno",
        name: "HUNO",
        url: "https://hawke.uno/",
        framework: Framework::Xbtit,
        default_enabled: false,
    },
    SiteDef {
        id: "hdtorrents",
        name: "HD-Torrents",
        url: "https://hd-torrents.org/",
        framework: Framework::Xbtit,
        default_enabled: false,
    },
    // --- 自研系统 ---
    SiteDef {
        id: "aither",
        name: "Aither",
        url: "https://aither.cc/",
        framework: Framework::Custom,
        default_enabled: false,
    },
    SiteDef {
        id: "bitporn",
        name: "BitPorn",
        url: "https://bitporn.eu/",
        framework: Framework::Custom,
        default_enabled: false,
    },
    SiteDef {
        id: "gpw",
        name: "海豹",
        url: "https://greatposterwall.com/",
        framework: Framework::Custom,
        default_enabled: false,
    },
    SiteDef {
        id: "lst",
        name: "LST",
        url: "https://lst.gg/",
        framework: Framework::Custom,
        default_enabled: false,
    },
    SiteDef {
        id: "myanonamouse",
        name: "MyAnonamouse",
        url: "https://www.myanonamouse.net/",
        framework: Framework::Custom,
        default_enabled: false,
    },
    SiteDef {
        id: "ourbits",
        name: "我堡",
        url: "https://ourbits.club/",
        framework: Framework::Custom,
        default_enabled: false,
    },
    SiteDef {
        id: "sjtu",
        name: "葡萄",
        url: "https://pt.sjtu.edu.cn/",
        framework: Framework::Custom,
        default_enabled: false,
    },
    SiteDef {
        id: "torrentleech",
        name: "TorrentLeech",
        url: "https://www.torrentleech.cc/",
        framework: Framework::Custom,
        default_enabled: false,
    },
];

pub fn builtin_sites() -> &'static [SiteDef] {
    SITE_TABLE
}

/// 从 URL 里取 host（小写）
pub fn url_host(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
}

/// 按 host 在内置表里认站
pub fn site_def_by_host(host: &str) -> Option<&'static SiteDef> {
    let h = host.to_lowercase();
    SITE_TABLE
        .iter()
        .find(|s| url_host(s.url).as_deref() == Some(h.as_str()))
}

/// 按配置构造站点适配器（CLI 与 GUI 共用一份实现，防止「缺 cookie 警告」两边漂移）。
/// `filter` 非空时只构造这些站点 id；缺 cookie 的站通过回调点名（CLI 打 stderr、
/// GUI 进日志面板），请求照发——站点侧会给出 failed，账面原因才不会丢。
pub fn build_adapters(
    cfg: &crate::config::Config,
    filter: Option<&[String]>,
    mut on_missing_cookie: impl FnMut(&[String]),
) -> anyhow::Result<Vec<std::sync::Arc<nexusphp::NexusPhp>>> {
    use anyhow::Context as _;
    let allow: Option<std::collections::HashSet<&str>> =
        filter.map(|v| v.iter().map(|s| s.as_str()).collect());
    let mut out = Vec::new();
    let mut missing_cookie: Vec<String> = Vec::new();
    for s in &cfg.sites {
        if let Some(a) = &allow {
            if !a.contains(s.id.as_str()) {
                continue;
            }
        }
        let cookie = cfg.cookie_header_for(s.primary_host());
        if cookie.is_none() {
            missing_cookie.push(s.id.clone());
        }
        let mut adapter = nexusphp::NexusPhp::new(
            &s.id,
            &s.name,
            &s.url,
            s.search_url.as_deref(),
            cookie.as_deref(),
        )
        .with_context(|| format!("站点配置不合法：{}", s.id))?;
        adapter.user_agent = cfg.user_agent.clone();
        adapter = adapter
            .with_announce_hosts(s.announce_hosts.clone())
            .with_announce_url(s.announce_url.clone());
        out.push(std::sync::Arc::new(adapter));
    }
    if out.is_empty() {
        anyhow::bail!(
            "没有可参与的站点（检查 boxsend.json sourceSites 的 enabled/assist.siteIDs/pausedSites）"
        );
    }
    if !missing_cookie.is_empty() {
        on_missing_cookie(&missing_cookie);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_matches_swift_registry() {
        // 与 Swift `SiteRegistry.prioritySites` 条数一致（148）
        assert_eq!(SITE_TABLE.len(), 148);
        // 9 个优先站默认启用
        assert_eq!(SITE_TABLE.iter().filter(|s| s.default_enabled).count(), 9);
        // id 唯一
        let mut ids: Vec<&str> = SITE_TABLE.iter().map(|s| s.id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "站点 id 有重复");
    }

    #[test]
    fn framework_parse_and_capability() {
        assert_eq!(Framework::parse("NexusPHP"), Framework::NexusPHP);
        assert_eq!(Framework::parse("Unit3D"), Framework::Unit3D);
        assert_eq!(Framework::parse("custom"), Framework::Custom);
        assert!(Framework::NexusPHP.search_capable());
        assert!(!Framework::Unit3D.search_capable());
        assert!(!Framework::Blu.search_capable());
    }

    #[test]
    fn extra_announce_hosts_cover_real_aliases() {
        // 实机 42,190 条种子统计出的真实差异，别删（删了预筛会把已做种当缺口）
        assert!(extra_announce_hosts("luckpt").contains(&"tracker.luckpt.de"));
        assert!(extra_announce_hosts("chdbits").contains(&"chdbits.xyz"));
        assert!(extra_announce_hosts("qilin").contains(&"tracker.hdkyl.in"));
        assert!(extra_announce_hosts("baozi").contains(&"tracker.t-baozi.cc"));
        assert!(extra_announce_hosts("novahd").contains(&"tracker.novahd.top"));
        assert!(extra_announce_hosts("xingyunge").contains(&"tracker.xingyungept.org"));
        assert!(extra_announce_hosts("ptlgs").contains(&"ptl.gs"));
        assert!(extra_announce_hosts("pthome").contains(&"t.pthome.org"));
        assert!(extra_announce_hosts("agsvpt").contains(&"tracker.agsvpt.cn"));
        assert!(extra_announce_hosts("cspt").contains(&"tracker.cspt.cc"));
        assert!(extra_announce_hosts("tey").contains(&"tracker.tey.cc"));
        assert!(extra_announce_hosts("hdsky").is_empty());
    }

    #[test]
    fn only_verified_sites_allow_hash_rewrite() {
        assert!(allows_target_hash_rewrite("ptba"));
        assert!(allows_target_hash_rewrite("cspt"));
        assert!(allows_target_hash_rewrite("dubhe"));
        assert!(allows_target_hash_rewrite("ptsbao"));
        assert!(allows_target_hash_rewrite("crabpt"));
        assert!(allows_target_hash_rewrite("hdbao"));
        assert!(allows_target_hash_rewrite("tey"));
        assert!(allows_target_hash_rewrite("oshen"));
        assert!(!allows_target_hash_rewrite("hdvideo"));
        assert!(!allows_target_hash_rewrite("qilin"));
    }

    #[test]
    fn host_lookup() {
        assert_eq!(site_def_by_host("hdsky.me").unwrap().id, "hdsky");
        assert_eq!(site_def_by_host("www.uhddolby.com").unwrap().id, "hddolby");
        assert!(site_def_by_host("nonexistent.example.com").is_none());
    }

    #[test]
    fn host_matches_exact_and_subdomain() {
        // 精确相等
        assert!(host_matches("hdsky.me", "hdsky.me"));
        // tracker 在子域（NexusPHP 常见：tracker.xxx.com / t.xxx.org）
        assert!(host_matches("tracker.pterclub.com", "pterclub.com"));
        assert!(host_matches("t.abc.org", "abc.org"));
        // `www.` 不改变站点身份：页面带 www，announce 在裸域或另一子域都算同站
        assert!(host_matches("tracker-public.tjupt.org", "www.tjupt.org"));
        assert!(host_matches("www.tjupt.org", "tjupt.org"));
        assert!(host_matches("www.oshen.win", "oshen.win"));
        // 跨域不算；反方向（页面域是 tracker 的「父」才对，反过来不成立）
        assert!(!host_matches("tracker.pterclub.com", "pterclub.net"));
        assert!(!host_matches("pterclub.com", "tracker.pterclub.com"));
        // 空值永不匹配
        assert!(!host_matches("", "hdsky.me"));
        assert!(!host_matches("hdsky.me", ""));
    }
}
