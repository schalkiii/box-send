# 并行开发计划（2026-11-22）

> **执行结果（11-22 下午）**：P1/P2/P3 已在主工作区串行完成（未另开 worktree/分支），
> 产物为 `scripts/seeding-audit.ps1`、`doc/SEEDING-AUDIT.md`、`scripts/site-matrix.ps1`、
> `doc/SITE-ADAPTATION-MATRIX.md`，以及 `models.rs`/`manifest.rs`/`bencode.rs` 的纯函数单测。
> 验收：fmt/clippy 零告警，`cargo test --workspace` 196 单测 + 24 e2e + 2 CLI + 3 GUI 全绿。
> 同时补齐了主 Agent 遗留的 BUG-M7（审阅产物直发）与 S-16（站点写接口级 posted/blocked e2e）。
> P4（`bsend doctor`）仍暂缓。

目标：主 Agent 继续修转种/验证/GUI，另一个 AI Agent 并行做**不与主 Agent 写集相交**的
验证与增强任务。所有任务都**不真实发种、不改配置、不打印凭据、不 push**。

## 1. 当前主 Agent 的写作用域（其他 Agent 不要碰）

| 范围 | 文件 |
| --- | --- |
| 转种核心 | `rust/crates/bsend/src/reseed.rs`、`qbittorrent.rs`、`index.rs`、`layout.rs` |
| 转种测试 | `rust/crates/bsend/tests/coverage_e2e.rs` |
| GUI | `rust/crates/bsend-gui/src/main.rs` |
| 文档 | `doc/RESEED-SEMANTICS.md`、`doc/DATA-FILES.md`、`doc/AUDIT-REPORT-*.md`、`rust/README.md`、`CHANGELOG.md` |

## 2. 并行任务（按优先级）

### P1 转种/辅种只读审计脚本（推荐先做）

- **产出**：
  - `scripts/seeding-audit.ps1`（新增）
  - `doc/SEEDING-AUDIT.md`（新增，中文使用说明 + 字段解释）
- **写作用域**：仅以上两个新文件；**不改任何 Rust 源码、不改配置**。
- **功能**：
  1. 读 `boxsend.json`（参数 `-Config`，默认 `%LOCALAPPDATA%\BoxSend\boxsend.json`）
     与同目录 `coverage.json`、`reseed-log.jsonl`；
  2. 通过 qB WebAPI（地址/账号从配置读取，**绝不打印**）列出：
     - 格子状态为 `reseed_draft / verifying / same_hash_skip / failed` 的条目（最多 50 条）；
     - 最近 7 天 `reseed-log.jsonl` 中 `seeded=false` 的记录（按目标站分组计数）；
     - qB 中带 `Boxsend` 标签但 `progress < 1` 的种子（最多 50 条）。
  3. 控制台输出摘要表；`-Json` 输出机器可读；`-OutFile` 写 Markdown。
- **验收**：
  - `pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/seeding-audit.ps1 -Config <路径>`
    能跑通并输出摘要；无凭据/密码/cookie 出现在输出；
  - 文件缺失时给清晰提示而不是抛栈；
  - 脚本不超过 300 行，注释中文。
- **禁止**：调用任何发种/上传接口；修改 qB；写 `coverage.json`。

### P2 站点适配矩阵（只读汇总）

- **产出**：
  - `scripts/site-matrix.ps1`（新增）
  - `doc/SITE-ADAPTATION-MATRIX.md`（新增）
- **写作用域**：仅以上两个新文件。
- **功能**：
  - 调用 `rust\target\release\bsend.exe --config <cfg> sites --json`（只读）；
  - 读 `%LOCALAPPDATA%\BoxSend\formcheck.json`；
  - 生成 Markdown 矩阵：站点 id/名称、enabled、paused、reseedPaused、cookie 数、
    pieces 能力、drill 结论与原因、formcheck 字段/下拉/缺口；
  - 顶部汇总：启用站数、drill ok/fail/gap/未测、4 类已知 gap 原因。
- **验收**：脚本能重复生成同一份文档；不修改 `formcheck.json`；不联网（只跑本地命令）。
- **禁止**：发种、上传、改配置；不要动 `rust/`。

### P3 纯函数单测补齐（Rust，独立 worktree）

- **产出**：只改这三个文件的 `#[cfg(test)]`：
  - `rust/crates/bsend/src/manifest.rs`
  - `rust/crates/bsend/src/bencode.rs`
  - `rust/crates/bsend/src/models.rs`
- **写作用域**：仅以上三个文件；**不改生产逻辑**。
- **内容**：先读实现，补边界用例（空文件清单、Unicode 路径、单文件/多文件根名、
  异常 bencode、空 token 等）；每个文件至少 3 条有意义的断言。
- **验收**：`cargo test -p bsend` 全绿；`cargo clippy -p bsend --all-targets -- -D warnings`
  零告警；`git diff --stat` 只包含这三个文件。
- **禁止**：改函数签名/行为；改 `reseed.rs`、`index.rs`、`coverage.rs`、GUI；跑真实站点。

### P4（暂缓）`bsend doctor` 诊断命令

- 依赖 `bsend-cli/src/main.rs`（与后续 CLI 改动可能冲突），本轮不排。

## 3. 协作方式

1. 每个任务独立分支：`parallel/p1-seeding-audit`、`parallel/p2-site-matrix`、
   `parallel/p3-pure-tests`；**不要 push**，完成后由主 Agent 串行合并。
2. P1/P2 在同一工作区做（只写 `scripts/`、`doc/` 新文件）；P3 建议用
   `git worktree add ..\box-send-p3 parallel/p3-pure-tests` 隔离，避免与主 Agent 的
   `cargo test` 互相等待锁。
3. 汇报格式：改动的文件、运行的命令、结果摘要（通过/失败数）、风险与未决项。
4. 任何需要真实发种、改配置、读凭据的步骤一律停止并上报。

## 4. 可直接粘贴的提示词

### P1 提示词

```text
你在 Windows 的 BoxSend 仓库工作：C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34。
这是一个 PT 转种/辅种工具，Rust 工作区在 rust/，运行时数据在
C:\Users\Schal\AppData\Local\BoxSend（boxsend.json、coverage.json、reseed-log.jsonl）。

任务：新增一个只读审计脚本 scripts/seeding-audit.ps1 和说明文档
doc/SEEDING-AUDIT.md，回答「站点上发了、qB 里有没有真的在辅种」。

要求：
1. 只用 PowerShell 7（pwsh），参数 -Config（默认 %LOCALAPPDATA%\BoxSend\boxsend.json）、
   -Days 7、-Json、-OutFile、-Limit 50。
2. 读取配置里的 qB 地址/用户名/密码并登录 WebAPI；绝对不要在输出里打印密码、cookie、
   passkey。只读接口：/api/v2/torrents/info、/api/v2/torrents/tags 等。
3. 汇总三类问题：
   a. coverage.json 中 status 为 reseed_draft / verifying / same_hash_skip / failed 的格子；
   b. reseed-log.jsonl 最近 N 天 seeded=false 的记录（按 target_site 计数）；
   c. qB 中带 Boxsend 标签且 progress < 1 的种子。
4. 控制台输出摘要；-Json 输出 JSON；-OutFile 写 Markdown。文件缺失时提示清楚。
5. 只写 scripts/seeding-audit.ps1 与 doc/SEEDING-AUDIT.md 两个新文件；
   不改 rust/、不改配置、不调用任何上传/发种接口、不 push。
6. 完成后运行脚本一次（只读），把输出摘要贴回来；报告改动文件与命令。
```

### P2 提示词

```text
你在 Windows 的 BoxSend 仓库工作：C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34。
Rust CLI 已构建在 rust\target\release\bsend.exe；运行数据在
C:\Users\Schal\AppData\Local\BoxSend（boxsend.json、formcheck.json）。

任务：新增 scripts/site-matrix.ps1 和 doc/SITE-ADAPTATION-MATRIX.md，生成站点适配矩阵。

要求：
1. 调用 `rust\target\release\bsend.exe --config <cfg> sites --json`（只读）拿站点总账；
   读 formcheck.json（若不存在则标注「未测」）。
2. 生成 Markdown：每个站点一行（site/name/enabled/paused/reseedPaused/cookies/pieces/
   drill/drillReason/formcheck 摘要），顶部汇总启用数、drill ok/fail/gap/未测、
   已知 gap 原因（musopia/muxuege/ptfans/hdcity）。
3. 脚本可重复运行、输出稳定；不改 formcheck.json、不联网、不发种、不改配置。
4. 只写 scripts/site-matrix.ps1 与 doc/SITE-ADAPTATION-MATRIX.md 两个新文件；不 push。
5. 完成后运行一次，报告生成的行数与汇总数字。
```

### P3 提示词

```text
你在 Windows 的 BoxSend 仓库工作：C:\Users\Schal\WorkBuddy\2026-11-17-20-52-34。
Rust 工作区 rust/，核心库 crate 是 bsend。

任务：只给以下三个文件补单元测试（#[cfg(test)]），不改生产逻辑：
- rust/crates/bsend/src/manifest.rs
- rust/crates/bsend/src/bencode.rs
- rust/crates/bsend/src/models.rs

要求：
1. 先读实现，理解现有边界（空文件清单、Unicode 路径、单文件/多文件根名、异常 bencode、
   空 token 等），每个文件至少 3 条有意义的断言。
2. 不要改函数签名、行为、依赖；不要碰 reseed.rs/index.rs/coverage.rs/GUI/文档。
3. 建议在独立 worktree 工作：
   git worktree add ..\box-send-p3 -b parallel/p3-pure-tests
   然后 cd ..\box-send-p3\rust 工作。
4. 验收：cargo test -p bsend 全绿；cargo clippy -p bsend --all-targets -- -D warnings 零告警；
   git diff --stat 只包含上述三个文件。
5. 不要 push；完成后报告新增测试数、运行结果与改动文件。
```

## 5. 合并与验收（主 Agent）

- P1/P2：检查脚本只读、无凭据输出，运行一次确认摘要合理后合入。
- P3：确认 diff 仅测试、`cargo test --workspace` 全绿后合入。
- 合并顺序：P1 → P2 → P3；每合一个跑一次 `cargo fmt --check`、`clippy -D warnings`、
  `cargo test --workspace`，避免并行改动互相污染。
