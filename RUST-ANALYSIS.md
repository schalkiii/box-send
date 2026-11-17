# 用 Rust 重写：收益分析与路线（v1）

## 结论先行

**短期（辅种功能落地这一两年内）：不值得整体重写。** 辅种方案（见 `COVERAGE-PLAN.md`）
是核心逻辑改造，Swift 完全胜任；此刻重写等于把功能推迟一个季度以上，还要冒着
双栈维护适配器的风险。

**长期若产品形态转向「无人值守守护进程 + 单二进制分发」：值得，且只值得从核心库开始、
按站点流量逐站搬迁。** 触发条件见第五节——满足之前不动工。

这个判断的依据展开如下。

## 一、重写的对象是什么（现状盘点）

| 部分 | 规模 | 说明 |
| --- | --- | --- |
| `BoxSendKit` 核心库 | 51 文件 / 22,214 行 | 纯 Foundation、零第三方依赖；其中 `Sites/` 23 文件 / 10,777 行是 **148 站 × 10 个适配器框架** |
| mac 界面（SwiftUI/AppKit） | 3 文件 / 5,134 行 | `Views.swift` 3,099 + `AppModel.swift` 2,009 |
| Windows 界面（WPF + P/Invoke） | 42 文件 / 8,433 行 | C ABI DLL（`boxsend-core.dll`）+ JSON 桥，600ms snapshot 轮询 |
| CLI / 桥 | 4 文件 / 637 行 | CLI 在 Debian 无头可跑；`deploy/` 有 systemd 单元 |
| 测试 | 66 文件 / 12,092 行 | 646 个全绿，fixtures 驱动（`Tests/Fixtures`，站点页面存档当断言） |

关键观察：**「厚核心、薄外壳」已经成立**（Windows 壳只有核心的 1/5），界面不是重写的主因；
真正的资产和风险都在 `Sites/` 那 1.1 万行站点怪癖里。

## 二、Rust 真实能解决的痛点

按 `PORTING-WINDOWS.md` 踩坑记录逐条对照，收益从大到小：

1. **Windows 构建与分发**。现状：CI 自装 2GB Swift 工具链（25 分钟硬闸）、WiX Burn
   静默参数、`SDKROOT` 从注册表补、`@_cdecl` 符号要逐个 `-Xlinker /EXPORT:`、
   Swift 运行时部分 DLL 是运行期 `LoadLibrary`（导入表看不见，所以必须有「真调一次」闸门）、
   DLL 拍平 + 撞名规则、自包含包 161MB/465 文件。Rust 下这些**全部消失**：
   GH Actions 预装工具链、一条 `cargo build --release` 出静态单文件（10–20MB）、
   任何主机可交叉编译三平台。
2. **无人值守形态**。辅种/辅种天然是「盒子/VPS 上 24×7 跑、定时增量、预算限流」的守护进程。
   Swift on Linux 能跑（`deploy-debian.sh` 证明了），但每台机器都要装整套工具链，
   生态边缘；Rust 出单二进制 + scratch Docker，`systemd`/容器分发是常态。
3. **并发模型**。`AssistPipeline.run` 目前全串行（站点外层 × 种子内层），辅种方案里
   「每站一个 worker、站内保节奏、全局并发上限」在 Swift 里能用 DispatchQueue 硬写，
   但 tokio 的每站 task + `governor` 令牌桶是更自然的表达，且不会把 600ms 轮询桥
   改成事件流时再难一次。
4. **事件桥**。现在 UI 靠 600ms snapshot 轮询，是因为 C ABI 上做回调太脆。
   Rust 守护进程直接 SSE/WebSocket，界面（无论 Tauri 还是保留的桌面壳）实时拿事件。
5. **生态**。reqwest（HTTP+cookie jar）、scraper（HTML）、serde、rusqlite（状态入库，
   万级内容索引比 JSON 文件稳）、bendy/bencode 系、aes/cbc（CookieCloud 兼容）全成熟。

**不是理由的两件事**：性能（IO 密集爬虫，Swift 不慢）与内存安全（两边都是安全语言）。
别用这两条说服自己。

## 三、Rust 解决不了 / 会变糟的

1. **适配器是皇冠上的宝石，也是最贵的重写**。10,777 行里堆的不是框架代码，是
   逐站实测的怪癖：海胆的 `group_id` 结果页要逐条打开复核、可乐 Vue SPA 四步发种、
   馒头 API 字典、NexusPHP 全字段动态匹配、`uploadFilename` ASCII 洗名（中文名被站方拒）、
   hdtime 发种 2 秒后才有下载入口……重写 = 全部重踩一遍。更糟的是站点会改版：
   迁移期内同一站要维护 Swift + Rust 两份适配器，churn 最高的地方成本翻倍。
2. **两个 GUI 都得重做**。Rust 没有与 SwiftUI/WPF 对等的原生 UI。现实选择只有
   Tauri（Web UI，等于重写界面层）或 CLI+Web 优先（放弃原生界面）。
3. **单人作者的学习与停滞成本**。async/借用检查的现实磨合期以月计；这期间辅种、
   站点改版跟进全部变慢——而「站点改版跟得快」正是这个项目的生命力。
4. **已验证的东西要重新验证**。646 个测试、四道构建闸门、两端行为一字不差的对齐审计，
   都是重写后要重新挣回来的。

## 四、决策矩阵

| 维度 | 留在 Swift | 重写 Rust |
| --- | --- | --- |
| 辅种功能落地速度 | ✅ 现有管道直接改 | ❌ 先还 3 万行债 |
| Windows CI 痛苦 | ❌ 维持现状 | ✅ 根除 |
| 分发体积/方式 | ❌ 161MB 自包含 | ✅ 单文件 10–20MB |
| 盒子无人值守 | ⚠️ 能跑但部署重 | ✅ 二进制 + 容器 |
| 站点改版响应 | ✅ 一处改 | ⚠️ 迁移期两处改 |
| 界面 | ✅ 两端原生保留 | ❌ 重做（Tauri/Web） |
| 并发辅种 | ⚠️ 能写但不优雅 | ✅ tokio 自然 |
| 测试资产 | ✅ 直接用 | ❌ 逐条移植 |

## 五、触发条件（什么时候值得动工）

满足**任意两条**再启动：

1. 无人值守盒子守护进程成为主产品形态（Web/远程优先，桌面 GUI 沦为附属）；
2. Windows CI 时间与翻车成为每月主要维护成本（现在每次 25 分钟硬闸 + 偶发工具链变更）；
3. 需要多机/集群调度（多台盒子协同辅种，状态与锁需要真正的服务端）；
4. 发布渠道要求（如希望进 Homebrew core / Docker Hub / 单文件绿色版）成为硬需求。

## 六、如果做：路线（R0–R5）

原则：**新功能单边（只写 Rust），旧功能按流量搬家，同一站点同一时刻只有一份适配器实现。**

| 期 | 内容 | 规模 | 闸门 |
| --- | --- | --- | --- |
| R0 尖刀 | 移植纯逻辑层：`Bencode`、`AssistMatcher`、`SiteThrottle`、`ContentIndex`（辅种 C0 产物）到 `boxsend-core` crate；fixtures 测试先行 | 1–2 周 | Rust 版 fixtures 测试与 Swift 版同一份输入全绿 |
| R1 网络 | reqwest + cookie jar；两个适配器：NexusPHP 通用 + Unit3D（覆盖站点最多 + 字段最规整）；假服务器集成测试 | 2–3 周 | 用存档 HTML 回放，输出与 Swift 版逐字段一致 |
| R2 状态 | rusqlite 建库；`state.json`/`coverage.json` 迁移工具；serde 兼容现有 `boxsend.json`（老配置原样读入） | 1–2 周 | 双向迁移零丢失 |
| R3 守护进程 | **辅种流水线只写 Rust**（这是避免双实现的关键决策）：daemon + HTTP API 对齐 Web 控制台 `/api/*`；SSE 事件流；systemd/Docker 镜像 | 3–4 周 | 盒子上无头跑通辅种 dry-run 与实跑 |
| R4 适配器搬家 | 按站点流量排序（top 20 站覆盖绝大多数转种流量），逐站移植：fixtures 为闸、移一个切一个、Swift 侧随切随删 | 每站 0.5–2 天 × N | 每站移植后真发一单验证 |
| R5 界面 | Tauri 壳复用 Web 控制台 UI；WPF 退役；mac SwiftUI 最后处理（或保留、改为调 daemon API） | 3–4 周 | 两端教程文案一字不差（沿用 `TutorialText` 钉测试的思路） |

总量级：**3–5 个月单人全职**到 R5（不含适配器全量 148 站——按流量搬，剩下的站留在
Swift 长尾或等社区）。R0–R2 合计约 1 个月，之后每期独立可停，不影响线上 Swift 版。

**与辅种方案的衔接**：辅种在 Swift 里先做**不亏**——`ContentIndex`/`SourceResolver`/
判定器是纯逻辑，恰是 R0 最先搬的模块，且辅种的使用反馈会直接修正 Rust 版的设计。
反过来「先重写再辅种」会让功能晚一个季度、且没有真实数据校验新架构。

## 七、给当下这一轮的建议

1. 辅种按 `COVERAGE-PLAN.md` 在 Swift 里做，C0 的 `ContentIndex` 写成**纯输入输出**
   （不碰 HTTP、不碰 Foundation 之外的类型），就是给未来 R0 预制的移植件。
2. 站点 worker 并发（C2）认真做——它是 Swift/Rust 两边都成立的结构改造，
   Rust 化时直接换运行时不换逻辑。
3. Web 控制台（C5）当成一等公民投资：它是将来 Rust daemon 的 UI 雏形，
   现在每投一分钱都保值。
4. 每半年对照第五节触发条件复查一次；不满足就继续 Swift。
