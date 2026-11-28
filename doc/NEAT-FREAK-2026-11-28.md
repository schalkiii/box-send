# 洁癖收尾（2026-11-28）

> 范围：当前项目 `boxsend`，代码基线 `master` 的 `51e4792`；本文档改动随 `4eaa556` 提交。只读盘点 + 文档/规则同步；清场候选只列不删，等用户看完汇报后明确确认。
> 当前测试基线 560 条；代码提交 `eaef101` 的 CI 四 job 全绿，`51e4792` 只改文档。

## 一、六个事实面

| 事实面 | 状态 | 证据 / 说明 |
|---|---|---|
| 代码 | `verified-current` | `master 51e4792`；全量 560 条、fmt、clippy 通过；最近代码提交 `eaef101`，CI run 46120088570 四 job success |
| 运行态 | `verified-current` | 索引 3,388 段内容、42,470 条成员种子；`bsend stats` 44 个站点、157 条；覆盖分布见下 |
| 文档 | `changed-and-verified` | `README` 文档入口、当前基线、`TESTING` 560 条和 CLI 24 条已同步；历史审计/测试报告加了“历史”标注 |
| 规则 | `changed-and-verified` | `AGENTS.md` 的 560 条、tracker 拒客户端规则已在上一轮落地；本轮把推送命令改成当前可用的代理写法 |
| 记忆 | `not-applicable` | 本项目没有可手工维护的项目记忆文件；平台长期偏好只读，不在本次范围 |
| 工作区 | `pending` | `artifacts/` 564 文件 / 86.7 MB；根目录无未跟踪残留；没有本地/远端多余分支；清场候选见第四节 |

## 二、运行态快照

`bsend status`（2026-11-28 下午）：

```text
3,388 段内容、42,470 条成员种子
归档倒计时中（下载器里已消失）：7
源种已定位：96
覆盖：已在做种 29773、已辅种 27、已转种 126、校验中 8、其他版本 21、
同 hash 跳过 1、站内没有 20、待找源 54、失败 23、转种草稿 31、
已跳过 24、待走候选入口 1、等站点审核 39、站上已找不到 2
```

`bsend stats`：44 个站点、157 条；最近一条是 ptzone [194730](https://ptzone.xyz/details.php?id=194730)。

**需要继续等的外部项**：39 条等站点审核；8 条 `verifying` 已挂回、校验 100%，但 tracker 直接回 `Banned Client` / `Anti-Cheater`，当前 qBittorrent 5.3.0 被 hdvideo、ptzone、sbpt、pthome 禁用；2 条站上已找不到（qilin 479362、panda 141276）是终态；23 条 `failed` 是历史失败备选，不是待发队列。

## 三、本轮改动

- `README.md`：补 2026-11-28 当前基线（147 站 / 46 站白名单 / 44 站 157 条 / 560 条测试）；文档表补 `SITE-REVIEW-2026-11-28`、`REVIEW-RECURRENCE-2026-11-28`、`NEAT-FREAK-2026-11-28`，把 r3 审计和 11-27 测试报告标成历史证据。
- `AGENTS.md`：推送命令从“强制直连”改成“优先本机 7890 代理，代理挂了再直连”；直连现在会超时/连接重置，旧写法本轮实测失败。
- `doc/TESTING.md`：当前基线 536 → 560，拆清核心 481、`coverage_e2e` 35、`other_version` 2、`skip_keywords` 2、CLI 24、`cli_smoke` 3、GUI 13；CLI 单测 20 → 24。
- `doc/TEST-REPORT-2026-11-27.md`：顶部加“历史报告，当前 560 条”标注。
- `doc/AUDIT-REPORT-2026-11-27-r3.md`：顶部加“历史审计，当前代码 `51e4792`、560 条”标注。
- `doc/NEAT-FREAK-2026-11-27.md`：顶部加“历史报告，清场候选已处理”标注。
- `doc/SITE-REVIEW-2026-11-28.md`：`SITES.md` 的过时精确字节数改成“13 万字节级别”。

## 四、待确认清场候选（一个都没删）

### 4.1 `artifacts/` 里的站点种子（含 passkey）

| 文件 | 大小 | 说明 |
|---|---:|---|
| `artifacts/hdvideo-26007.torrent` | 44,592 | 2026-11-28 补挂证据，站点可重新下载 |
| `artifacts/hdvideo-74646.torrent` | 269,022 | 同上 |
| `artifacts/pthome-285043.torrent` | 213,629 | 同上 |
| `artifacts/sbpt-21692.torrent` | 23,686 | 同上 |
| `artifacts/sbpt-21693.torrent` | 13,062 | 同上 |
| `artifacts/sbpt-21786.torrent` | 46,263 | 同上 |

这些是实机证据，`.gitignore` 已整目录挡住，不会入库；确认不再复核后可删。

### 4.2 `artifacts/covtest/` 旧覆盖副本

- `artifacts/covtest/BoxSend/coverage.json` 和 `.bak-20261128-142418`，合计约 45.9 MB，已被现役 `C:\Users\Schal\AppData\Local\BoxSend\coverage.json` 和后续备份取代。

### 4.3 重复的 GUI 截图

- `artifacts/bsend-gui-*.png` 里 11 张 1.7–2.4 MB 的整屏截图是点击/置前尝试的过程图；最终可读图是 `bsend-gui-poststats-20261128-156.png`。若要留证据，保留最终图即可。

### 4.4 根目录和分支

- 根目录无未跟踪残留；`1.webp` 是 `scripts/make-app.sh` 的应用图标源，保留。
- 本地和远端只有 `master`；`main`、`rust/coverage` 已不在，上一轮的分支清理已完成。

> 以上删除候选未执行；按洁癖规则，等用户看完汇报后明确确认再删，删完重新审计一次。

## 五、遗留 / 不在本轮范围

- `pending_review` 39 条、`verifying` 8 条、`gone_from_site` 2 条、`failed` 23 条：都是运行态现状，不是本轮文档收尾能消掉的；处理口径见 `ROADMAP.md` 和 `AGENTS.md`。
- `pending_offers` 1 条：qingwa 的波洛合集被站点判定种子过大，要走 `offers.php` 候选入口。
- `doc/` 里按日期命名的报告全部保留作证据；没有把历史报告删掉或合并成第二套真相。
