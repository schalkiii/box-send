# 只读巡检：覆盖率异常 / 未成种 / 下载器卡种

`scripts/seeding-audit.ps1` 是一个**只读**排障入口：不改配置、不写覆盖率、不发站点请求，
只在本地读 `coverage.json`、`reseed-log.jsonl`，并向 qBittorrent 读一次种子列表。

## 为什么需要它

自动发种跑过几轮后，最容易出现三类「看起来没事、其实没做成」的情况：

1. 覆盖率格子停在 `failed` / `reseed_draft` / `same_hash_skip`，没人翻；
2. `reseed-log.jsonl` 记了发布成功，但 `seeded=false`（本机没挂上目标种）；
3. qB 里目标种带 `Boxsend` 标签，但 `progress < 1`（校验没过或没开始）。

这三类都不会让程序报错，只会让「站点有、本机没做种」悄悄积累。脚本把它们一次列出来。

## 用法

```powershell
# 默认 Markdown 输出到终端（回看 7 天）
pwsh scripts/seeding-audit.ps1

# 写文件 + 放宽到 14 天
pwsh scripts/seeding-audit.ps1 -Days 14 -Limit 100 -OutFile audit.md

# 机器可读
pwsh scripts/seeding-audit.ps1 -Json -OutFile audit.json
```

参数：

| 参数 | 默认 | 说明 |
| --- | --- | --- |
| `-ConfigPath` | `%LOCALAPPDATA%\BoxSend\boxsend.json` | 与 GUI 共用同一份配置 |
| `-Days` | 7 | 回看 `reseed-log.jsonl` 的天数（1–90） |
| `-Limit` | 50 | 每类明细最多列多少条；汇总计数不受限 |
| `-Json` | 关 | 输出 JSON 而不是 Markdown |
| `-OutFile` | 无 | 写文件；缺省打到 stdout |

## 输出怎么读

### 1. 覆盖率异常格

只收需要人看一眼的状态，`seeding / assisted / reseeded / miss` 不算问题：

| 状态 | 含义 | 典型处置 |
| --- | --- | --- |
| `failed` | 检索/门禁/上传/推种失败 | 看备注；cookie/权限/网络/规则逐项排 |
| `verifying` | 推送后没等到校验结果 | 重跑一轮，或去 qB 看进度 |
| `reseed_draft` | 转种草稿未发 | `bsend reseed --draft N` 审阅；确认后 `--yes` |
| `review_pending` | 站内命中但清单对不上 | 人工确认是否同内容 |
| `other_version` | 同片不同 pieces | 不做跨种，换源或放弃 |
| `same_hash_skip` | 与 qB 已有种同 hash | 不改原种；确需该站做种只能换源 |
| `wait_source` | 源站也定位不到源种 | 进待找源清单，补源后重跑 |

### 2. 最近 N 天未成种记录

逐行读 `reseed-log.jsonl`，只列 `seeded=false`。注意：

- 新鲜安装可能还没有这份日志（首次 `run_reseed` 后才生成）；
- `result=posted` 且 `seeded=false`，说明**站点已发布、本机没挂上**，优先查目标种下载与
  `AddLayout`（见 `RESEED-SEMANTICS.md` 第 3 节）；
- 按站汇总能快速看出「哪个站总在发布后失败」。

### 3. 下载器未做种

只挑 `tags` 含 `Boxsend` 且 `progress < 1` 的种子，列名称/进度/状态/hash 前 8 位。
排查顺序：文件是否下到预期目录 → qB 校验进度 → tracker 是否连通 → 站点是否已记做种。

qB 登录兼容两种回包：`200 Ok.` 与本地免密/已登录时的 `204` 空体；是否成功以随后的
`torrents/info` 为准。

## 安全边界

- 只调用 qB 的 `auth/login` 与 `torrents/info`；**不调用** add/delete/stop 等写接口。
- 不访问任何 PT 站点，不读取也不打印 cookie 值。
- 输出中的 URL 查询参数（`passkey` / `torrent_pass` / `authkey` / `token` 等）会被替换为 `***`。
- 唯一写操作是 `-OutFile` 指定的报告文件。

## 退出码

| 码 | 含义 |
| --- | --- |
| 0 | 巡检完成（有问题也会正常退出，问题看报告） |
| 1 | 脚本异常（配置缺失、JSON 解析失败等） |
| 2 | PowerShell 参数校验失败 |

## 与 `sites` / `report` 的分工

- `bsend sites` / `scripts/site-matrix.ps1`：站点档案与发布演练（`formcheck`），回答「这个站能不能发」。
- `bsend report`：覆盖率宏观报表，回答「覆盖了多少、缺多少」。
- 本脚本：回答「哪里卡住了、卡在哪一步」，面向排障而不是统计。
