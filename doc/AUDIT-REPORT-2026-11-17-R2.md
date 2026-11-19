# 审计报告 R2：Rust 辅种引擎（rust/coverage @ 949ad91）

第二轮审计（第一轮见 `AUDIT-REPORT-2026-11-17.md`）。基准提交 `949ad91`（49 单测 + 9 e2e、
clippy/fmt 零告警、CI 双矩阵绿）。本轮逐文件全文重读全部 13 个模块 + CLI + e2e + CI +
两份设计文档，并对照 Swift 原版源码核移植完整度。

## 一、现状描述

- 规模不变：2 crate、约 7,800 行（不含 e2e 957 行）；第一轮 P1–P3 修复已全部落地。
- 上轮登记的 P4 项（merge_scan 双调、worker_loop 防御、bencode 深度）本轮重新评估后
  有 3 项降级为「低成本顺手修」。
- 本轮重点核了三类上轮未覆盖的面：**故障路径**（cookie 失效、站点限流、索引损坏）、
  **与 Swift 原版的行为对齐**（fetchHTML 的登录墙检测、SiteThrottle.penalize 的接线）、
  **对下载器的请求经济学**（每请求登录一次）。

## 二、问题清单

| # | 位置 | 问题 | 严重 | 影响 |
|---|------|------|------|------|
| 1 | `sites/nexusphp.rs` `get()`/`search()` | cookie 失效时站点 302 跳登录页，reqwest 跟随后拿到 200 登录页，解析不出详情锚 → `Ok(None)` → **记 Miss 负缓存**（目标站 7 天 / 源种 30 天）。Swift `HTTPClient.fetchHTML` 有检测（401/403 抛 cookieExpired + final URL 含 login 提示抛 cookieExpired），移植时漏了 | **中** | cookie 过期一轮把全站格子投毒成「站内没有」，最长 30 天不重查；且用户看不到「cookie 失效」提示 |
| 2 | `throttle.rs` `note_rate_limited` / `coverage.rs`、`resolver.rs` 检索错误路径 | **限流退避从未接线**：`note_rate_limited` 生产代码零调用（Swift 原版 `penalize` 同样零调用——死功能被忠实移植）。README「站点报限流 → 间隔翻倍退避」名不副实 | **中** | 站点回 429/503 后，该站仍按原节奏（默认 8s）被一直敲到轮末；PT 站对无视限流的行为会警告/封号 |
| 3 | `index.rs` `ContentIndex::load` | coverage.json 损坏（半截写入/磁盘问题）时 `.ok()` 链静默吞掉解析错误 → 当空索引用 → 下一轮 `save()` 用近乎全空的索引**覆盖**原文件 | **中** | 全部 settled 记录（已辅/已转/已否决）无声丢失且不可恢复；用户只会在下轮看到全量重搜重推 |
| 4 | `qbittorrent.rs` `login()` | 每个 API 调用前都 POST `/auth/login`（cookie store 里已有 SID 也照样登）。1000 条种子的扫描 = 1000+ 次多余登录请求 | **中** | 对 qB 请求数翻倍、qB 日志刷屏；登录虽幂等（204）但纯属浪费 |
| 5 | `index.rs` `save()` | `IndexFile { contents: self.contents.clone() }` 全量深拷贝再序列化 | 低 | 大索引（数千条目）时白付一倍内存与时间 |
| 6 | `index.rs` `merge_scan` | hash-only 失败路径 `files_of` 双调（271 行拿 None 建占位条目后，306 行对同一种子再调一次，必然还是 None）。上轮登记 P4 | 低 | qB API 波动时对下载器的请求翻倍 |
| 7 | `coverage.rs` `worker_loop` | 适配器查不到时 `continue` 静默丢整站队列（理论不可达）。上轮登记 P4 | 低 | 若未来调用方式变化会静默丢工作，无任何日志 |
| 8 | `bencode.rs` `skip_value` | 递归下降无深度上限 | 低 | 恶意/损坏 .torrent 深嵌套列表可栈溢出（真实种子深度 <10；风险低但修复廉价） |
| 9 | `config.rs` roundBudget 解析 | 负值不钳制：`roundBudget: -5` → 预算为负 → `acquire` 永远失败 → 整轮静默空转 | 低 | 配置笔误表现为「跑了但什么都没做」，无提示 |
| 10 | `rust/README.md` | 「站点报限流 → 该站间隔翻倍退避」在 #2 修复前与实现不符；cookie 失效的呈现（failed + 不记负缓存）未提登录页跳转场景 | 低 | 使用者预期偏差（随 #1/#2 修复转正，补一句登录页说明） |

**七维结论**：①文档一致性——问题 10；②文档缺漏——本维度未发现问题（上轮已补齐）；
③架构——无新问题；机制都在（throttle 退避、cookie store），缺的是接线，见问题 2；
④代码写法——问题 4/5/6/9；⑤测试覆盖率——问题 1/2/3 的故障路径（登录墙、限流、索引损坏）
零覆盖，本轮补；⑥测试严谨性——本维度未发现问题（e2e 断言到请求计数与索引终态）；
⑦工程规范——本维度未发现问题（CI/原子写/明文 JSON 维持良好）。

## 三、改进建议（做法 / 文件 / 验证）

### F1 登录墙检测（修问题 1，对齐 Swift fetchHTML）
- `nexusphp.rs` `get()`：响应状态 401/403 → `Err("站点回 HTTP {status}（多半是 cookie 失效）")`；
  重定向后的最终 URL（`resp.url()`）小写含 `login` / `enter.php` / `signin` / `auth/login`
  → `Err("被跳到登录页（cookie 可能已失效）")`。错误向上传播：目标站 → Failed（不记负缓存）、
  源种侧 → `clean=false`（不记负缓存）——正是现有错误路径的语义，无需改编排层。
- e2e 测试基建：`serve_conn` 对 3xx 状态把 payload 当 `Location:` 头发出（一行扩展），
  新增 `login_wall` 站点 mock（torrents.php → 302 /login.php → 200 登录页）。
- 验证：新 e2e `e2e_login_wall_is_failed_not_miss`（断言格子 Failed、note 含「登录」、
  源种不落负缓存）+ 全量 `cargo test`。

### F2 限流退避接线（修问题 2，让 README 变真）
- `nexusphp.rs` 新增 `pub fn error_is_rate_limited(e: &anyhow::Error) -> bool`
  （`{e:#}` 含 `HTTP 429` 或 `HTTP 503`）。
- `coverage.rs` `process_item` 两处检索 Err 分支、`resolver.rs` 两处检索 Err 分支：
  `if error_is_rate_limited(&e) { throttle.note_rate_limited(site_id); }` 后再走原有
  Failed/Miss{clean:false} 逻辑。
- 验证：单测 `error_is_rate_limited`；resolver 单测——mock 站检索回 429 错误 →
  `resolve_source` 后 `acquire_dur` 的等待 ≈ 2×interval（退避生效的直接证据）；
  e2e：429 站 mock → 格子 Failed 且 note 含 429。

### F3 索引损坏备份（修问题 3）
- `index.rs` `load()`：读到文件但解析失败 → 把原文件挪成 `<path>.corrupt`
  （已存在则覆盖）再返回空索引——状态可捞回，且坏文件不会被新 save 无声覆盖。
- 验证：扩展 `persistence_roundtrip`——写垃圾 → load 为空且 `.corrupt` 存在、内容是原垃圾；
  再次 load 仍为空；save 后新文件可正常读。

### F4 qB 登录缓存（修问题 4）
- `qbittorrent.rs`：`logged_in: AtomicBool`；`login()` 已登录直接返回；
  各响应状态检查处遇 403 清标记（qB 重启/SID 失效 → 下次调用自动重登，一轮内至多损一个动作）。
- 验证：e2e `e2e_assist_and_reseed_draft` 加断言 `POST /api/v2/auth/login` 恰好 1 次
  （修复前为一个 API 调用一次）；其余 e2e 全绿。

### F5 save 借用化（修问题 5）
- `index.rs`：`#[derive(Serialize)] struct IndexFileRef<'a> { contents: &BTreeMap<…> }`，
  `save()` 序列化借用结构，去掉 clone。
- 验证：`persistence_roundtrip` 全绿。

### F6 merge_scan 单次拉清单（修问题 6）
- `index.rs` `merge_scan`：记录本轮已对该种子调过且拿到 None 的 `files_tried` 标志，
  306 行的补摘要分支不再重复调用。
- 验证：新单测 `files_of_failure_calls_once`（计数闭包，断言 ==1）。

### F7 worker 丢队列改日志（修问题 7）
- `coverage.rs` `worker_loop`：适配器缺失分支发 `Event::Log`（带 site_id）再 continue。
- 验证：编译 + 全量测试（现有路径不触发）。

### F8 bencode 深度上限（修问题 8）
- `bencode.rs`：`skip_value` 内部改为带深度计数的递归，上限 64 层（真实种子 <10），
  超限返回 None。公开签名不变。
- 验证：单测 `deep_nesting_is_rejected`（300 层嵌套列表 → None，不炸）。

### F9 预算钳制（修问题 9）
- `config.rs`：`search_budget`/`source_budget` 解析后 `.max(0)`。
- 验证：config 单测——`roundBudget: -5` → 两个预算都是 0。

### F10 README 补正（修问题 10）
- 「站点报限流」行保持（F2 后为真）；辅种臂段补一句：cookie 失效（401/403 或被跳到
  登录页）按 failed 处理、不记负缓存。
- 验证：人工对照 F1/F2 实现。

## 四、优先级与并行性

- **F1 → F2**：都动 `nexusphp.rs` 且共享 e2e mock 扩展，**串行**；这两个是本轮的核心
  （行为正确性）。
- **F3 / F4 / F5 / F6**：分别只在 `index.rs` / `qbittorrent.rs` 内部，**可并行**
  （F3 与 F5 同文件不同函数，串行更稳）。
- **F7 / F8 / F9**：三处互不相干的小改，可并行。
- **F10** 最后做（依赖 F1/F2 定稿）。
- 每步后跑全量 `cargo test`；全部完成后 fmt → clippy -D warnings → test → 提交 →
  bundle 中继 push → CI 绿。

## 五、修复记录（2026-11-17 R2 处置结果）

F1–F10 全部落地（54 单测 + 11 e2e 全绿，fmt/clippy 零告警）：

- **F1 ✅** 登录墙检测（对齐 Swift fetchHTML）：`get()` 对 401/403 与「重定向后落在
  登录页」（final URL 含 login / enter.php / signin / auth/login）按 cookie 失效报错；
  `fetch_torrent_page`/`download_torrent` 的 403 分支收敛为 404 专用。
  e2e `e2e_login_wall_is_failed_not_miss`（检索 302 → 登录页：两格 failed、
  note 含「登录」、源种不落负缓存）。
- **F2 ✅** 限流退避接线：`nexusphp::error_is_rate_limited`（429/503）；
  `process_item` 与 `resolve_source` 共四处检索错误路径调用 `note_rate_limited`。
  单测 `rate_limited_error_backs_off_the_site`（429 后该站下次领号等 2×interval）、
  `rate_limited_error_detection`；e2e `e2e_rate_limited_maps_to_failed`。
- **F3 ✅** 索引损坏备份：`load` 解析失败把原文件挪成 `<path>.json.corrupt` 留证，
  不再被下一次 save 无声覆盖；`persistence_roundtrip` 扩展断言。
- **F4 ✅** qB 登录缓存：`logged_in: AtomicBool`，登录一次复用 SID；各响应路径遇 403
  清标记自动重登。e2e 断言一轮恰好 1 次 `POST /auth/login`。
- **F5 ✅** `save` 改用借用视图 `IndexFileRef` 序列化，去掉全索引深拷贝。
- **F6 ✅** `merge_scan` 失败路径 `files_of` 只调一次（`files_failed` 标志）；
  计数单测 `files_of_failure_is_called_once_per_torrent`。
- **F7 ✅** `worker_loop` 适配器缺失时发 `Event::Log` 再跳过，不再静默丢整站队列。
- **F8 ✅** bencode `skip_value` 深度上限 64；单测 300 层嵌套拒绝、正常深度照常。
- **F9 ✅** `assist.roundBudget` 负值钳 0；单测 `negative_round_budget_is_clamped`。
- **F10 ✅** README：限流行注明 429/503 自动触发、cookie 失效补登录页场景、
  移植清单补登录墙/单次登录、测试计数 54+11。
