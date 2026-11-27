# 洁癖收尾（2026-11-27）

> 范围：当前项目 `boxsend`，代码基线 `a5cd535`。只读盘点 + 文档/规则同步；清场候选只列不删，等用户看完汇报后明确确认。

## 一、六个事实面

| 事实面 | 状态 | 证据 / 说明 |
|---|---|---|
| 代码 | `verified-current` | `master` 在 `a5cd535`；生产 26,915 行、测试 16,403 行；535 条全绿；fmt/clippy 通过 |
| 运行态 | `verified-current` | city13 88467 tracker `status=2`、上传 227.16 MB、2 个下载者；hdvideo 79168 主 announce `status=2`、子 tracker `status=4`、0 上传；pthome 285040 明确 existed，未重复挂种 |
| 文档 | `changed-and-verified` | 535/79.87%/77% 已同步；`README`、`AGENTS` 的死引用已修；`SITES` hdvideo 已改 `reseeded`；新增 r3 审计本报告 |
| 规则 | `changed-and-verified` | `AGENTS.md` 补 `sites/nexusphp/` 子模块，删除“历史报告不再保留”的矛盾表述；ROADMAP 第 6 节并发风险改成当前写集 |
| 记忆 | `not-applicable` | 本项目没有可手工维护的项目记忆文件；平台长期偏好只读，不在本次范围 |
| 工作区 | `pending` | `artifacts/` 281 文件 / 28.42 MB；根目录 49 个 ignored 条目（其中 `.workbuddy/`、`rust/target/` 是工作/构建目录，不删，剩余 47 个一次性证据）；`main`、`rust/coverage` 已合入未删；清场待确认 |

## 二、当前代码/运行事实

- Rust 57 个 `.rs`、43,318 行：生产 26,915、内联测试 11,562、`tests/` 4,841；Markdown 33 个（含本轮新增的 r3、软件改进、洁癖报告）。
- 覆盖索引：3,388 段内容、42,470 条成员种子；覆盖分布“已辅种 27、已转种 117、待人工确认 19、其他版本 8、同 hash 跳过 1、站内没有 20、待找源 54”。
- 测试：核心 457、`coverage_e2e` 35、`other_version` 2、`skip_keywords` 2、CLI 23、`cli_smoke` 3、GUI 13，合计 535。
- CI：`0f1ff35` run #113 四 job success；`a5cd535` run #114 也已四 job success（2 分 35 秒）。

## 三、本轮改动

- `README.md`：补审计/测试报告入口；把“旧报告不再单独保留”改成“报告保留作证据，结论以最新一份为准”。
- `AGENTS.md`：模块清单补 `sites/nexusphp/{text,search,detail,upload}.rs`；规则尾部去掉与磁盘事实矛盾的“历史报告不再保留”；常用命令补推送直连写法（全局 `http.proxy` 会让 git TLS reset，用 `git -c http.proxy= -c http.sslBackend=openssl push origin master`）。
- `doc/SITES.md`：hdvideo 79168 结论从 `assisted` 改为 `reseeded`，tracker 更新为 `status=4`，补 city13 227.16 MB 复查。
- `doc/ROADMAP.md`：第 6 节并发风险改成 `reseed/` 与 `sites/nexusphp/` 的写集；补 4.8 后续优化（qB 分页、只读巡检、索引落盘基准、剩余 unwrap、清场）；hdvideo 状态同步。
- `doc/ARCHITECTURE.md`：测试基线 533 → 535。
- `doc/AUDIT-REPORT-2026-11-27-r2.md`：顶部标“历史版，现役见 r3”。
- 新增 `doc/AUDIT-REPORT-2026-11-27-r3.md`：七维审计、问题清单、S1–S7 建议、并行写集。
- 新增本报告 `doc/NEAT-FREAK-2026-11-27.md`。

## 四、待确认删除候选（一个都没删）

### 4.1 `artifacts/` 大件

| 文件 | 大小 | 理由 |
|---|---:|---|
| `artifacts/coverage-before-ghiblies.json` | 22.9 MB | 11-27 13:27 的旧覆盖备份，已被后续备份和现役 `coverage.json` 取代 |
| `artifacts/reseed.rs.bak-before-form-split` | 483 KB | 拆分前旧源码，已在 Git 历史 |
| `artifacts/reseed.rs.bak-before-audit-split` | 231 KB | 同上 |
| `artifacts/reseed.rs.bak-before-upload-post-split` | 201 KB | 同上 |
| `artifacts/bsend-cli-main.rs.bak-before-split` | 84 KB | 同上 |

### 4.2 含 passkey 的 torrent（8 个，均在 `artifacts/`）

- `artifacts/iloli-56513.torrent`
- `artifacts/railgun-58652.torrent`
- `artifacts/railgun-59010.torrent`
- `artifacts/u2-66255.torrent`
- `artifacts/gtk-evidence-20261127/gtk-7696.torrent`
- `artifacts/gtk-evidence-20261127/local-poirot.torrent`
- `artifacts/staging-xingtan-20261127/site-456304.torrent`
- `artifacts/staging-xingtan-20261127/xingtan-upload-hu8a.torrent`

> `Tests/BoxSendTests/Fixtures/` 里 3 个 `.torrent` 是测试固定数据，是否含真实 passkey 未逐一验证；不在本轮删除候选，先保留。

### 4.3 根目录 ignored 一次性证据（47 项，另 2 项工作/构建目录不删）

包括 `audit-final.txt`、`bsend-batch-2026-11-26.md`、`drafts-now.txt`、`followup-final.txt`、`formcheck-*.txt`、`gatecheck-*.txt/json`、`review*.txt`、`u2-*.html`、`workspace-test-20261127.txt`、`rust/*-final*.txt` 等。完整清单见 `artifacts/neat-inventory-20261127-r4.txt`。这些是过程日志，正式结论已进 `doc/`；保留最近一份门禁日志即可。`.workbuddy/` 和 `rust/target/` 不删。

### 4.4 分支

- `main` 已合入 `master`，可删本地 + 远端。
- `rust/coverage` 已合入 `master`，可删本地 + 远端。

> 删除分支不动提交历史；确认后执行 `git branch -d` 和 `git push origin --delete`。

## 五、保留现场

- `artifacts/` 最近的门禁日志：`workspace-full-test-20261127-final29.log`、`clippy-20261127-final28.log`、`fmt-check-20261127-final28.log`。
- 真实站点证据：`status-20261127-final30.txt`、`followup-hdvideo-20261127-final24.txt`、`followup-city13-20261127-final25.txt`。
- 覆盖数据备份：`C:\Users\Schal\AppData\Local\BoxSend\coverage.json.bak-20261127-2045`（本轮人工纠正前）。
- 清场候选未动；等用户明确确认后再删，并重新审计一次。
