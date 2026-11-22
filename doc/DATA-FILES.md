# 运行时数据文件（DATA-FILES）

> 全部位于 `dataDir`：取 `boxsend.json` 的 `dataDir` 字段；**未配置时退到配置文件
> 所在目录**（实机即 `C:\Users\Schal\AppData\Local\BoxSend`，因为配置在那里）。
> 代码里没有 `~/.boxsend` 的缺省逻辑——找不到文件时先确认配置的 dataDir。
> 本文件是手改/排障的契约：改前备份，改后保持引擎写出的完整字段形状。

## coverage.json（内容索引 + 站点覆盖账本）

- **写入者**：`bsend run` / `prime` / `reseed` / `autopost` / GUI 跑一轮（`plan` 是只读演练，
  不写账本；`scan` 只写索引、不动站点格子）。
- **结构**：`{ "contents": { "m:<hash16>": {...} } }`；每段内容含
  `name / totalSize / fileCount / manifestDigest / torrents{hash→meta} / trackerHosts /
  firstSeen / goneSince / coverage{siteId→格子} / source / forbidReseed`。
- **格子字段**：`status / at / url / note / verifyHash`，**五个字段必须齐全**（`url`/`note`
  没有 serde 缺省）。手改漏字段 → 整个文件解析失败 → 引擎按设计挪成
  `coverage.json.corrupt` 并从零重建（历史结论全丢）。
- **手改边界**：
  - 想「立刻重查某站某内容」= **删除该站格子**（无格 = 待查），不要写
    `status: "missing"`（枚举合法值是 `miss`，且 `miss` 是负缓存，TTL 内会被预筛跳过）。
  - 状态枚举 snake_case：`seeding / assisted / reseeded / verifying / review_pending /
    rejected / miss / wait_source / failed / reseed_draft / other_version / same_hash_skip`。
    `same_hash_skip`：目标种与下载器已有种子 info hash 相同，按策略不改动原种子（不补挂
    tracker、不打标签），该站暂不做种。
  - 改后可用 `bsend status` 验证能被正常解析。
- **损坏隔离**：解析失败时原文件改名 `coverage.json.corrupt` 留证，不会静默覆盖。

## auto-audit.jsonl（自动发种审计）

- **写入者**：`bsend autopost`（`bsend::autopost::append_audit`）。
- **每行一条 JSON**：`at`（写入时刻的 Unix 秒）、`site`、`key`（contentKey）、`result`
  （`posted / blocked / failed / review_failed / skipped_duplicate / other_version /
  existed_pending / same_hash_skipped`），可选 `url`（posted/existed）、`error`、`note`、
  `gates`（门禁矩阵）。
- **词义**：`skipped_duplicate` = 目标站已有同分片条目（含本工具此前发布的），跳过重传；
  **不是**站点拒绝上传。`same_hash_skipped` = 目标种与下载器已有种子同 hash，按策略不改动
  原种子。
- **用途**：日限额（按北京时间日界计 `posted`）、同草稿退避（拦截 24h / 失败 6h）。
- **手改边界**：只可追加/删除整行；**写失败会让本轮自动发种中止**（fail-closed），
  不要把它指向不可写路径。

## reseed-log.jsonl（转种动作日志）

- **写入者**：`bsend::reseed::run_reseed`（CLI `reseed --yes`、GUI「确认转种」、
  `autopost` 共用；每次转种写一行）。
- **每行一条 JSON**：`at`（结束时刻）、`started_at`、`key`、`name`、`source_site`、
  `source_url`、`target_site`、`result`（`posted / skipped_duplicate / other_version /
  existed_pending / same_hash_skipped / failed`）、`url`、`note`、`cell_status`、`seeded`。
- `seeded`：发种后是否确认已辅到 qB（格子落到 `reseeded/assisted/seeding` 才为 true；
  `verifying` 表示还在校验，需下一轮收尾）。
- **用途**：回答「站点上发了、qB 里为什么没有」——每次动作有开始/结束时间、源/目标站与
  结果；日志写失败只打 stderr，不影响发种结论（fail-open）。
- **手改边界**：只追加/删除整行，可整文件删除（不影响账本与限额）。

## pieces-cache.json（pieces-hash 通道缓存）

- **写入者**：`bsend::piecesapi::PiecesApi::save`（搜索/辅种命中后逐轮补全）。
- **结构**：`passkeys{host→passkey}`、`capabilities{siteId→ok|unsupported|unknown}`、
  `piecesHashes{infoHash→piecesHash}` 等。
- **手改边界**：可删条目触发重查；整文件可删（重建代价是若干次站点 API 调用，
  不是 qB 全量导出）。passkey 过期由 `refresh_passkey` 从 qB tracker 自动覆盖。
- 原子写（tmp + rename），中断不会写坏旧缓存。

## formcheck.json / gatecheck.json（演练结论）

- **写入者**：`bsend formcheck` / `bsend gatecheck`。
- **formcheck**：`{at, sample, results[]}`，每条 `{site, verdict, fields, selects,
  core[], soft[], error, at, restored?}`；`results` **按站点合并**：本次没跑的站保留最近结论。
- **gatecheck**：`[{site, ...门禁矩阵与官方预审结论}]`。
- 均可删（下次命令重算）；原子写，不会半截覆盖。

## screenshot-overrides.json（截图注入）

- **写入者**：独立抽帧工具（ffmpeg 抽帧 → 图床上传），人工/脚本维护。
- **结构**：`{ "<contentKey>": ["https://图床/…jpg", …] }`。
- **用途**：发布时注入简介（`reseed::extra_images_for`）；Imgur 注入图不参与死图探测。
- **手改边界**：只增删 URL 列表；保持公网可访问，勿写本地路径。

## cookies.json

- **写入者**：`bsend cookies --from cookiecloud --write`、GUI 的 CookieCloud 同步。
- **结构**：`{ "<host>": [{name, value, expirationDate?, ...}] }`（Chrome Cookie 形态）。
- **手改边界**：可整站删除触发重新同步；查询时已过期 cookie 会被过滤，不会发出。
  写回前引擎自动备份 `cookies.json.bak`。

## boxsend.json / boxsend.json.bak-*

- **写入者**：用户/GUI 配置编辑；引擎只读（`bsend --config` 指定）。
- **手改边界**：改前备份；`assist` 段全部键见 `Config/boxsend.example.json`。
  负值会被钳制（预算/超时），`publishEnabled` 缺省 false。
- 引擎读入失败会报错退出，不会用半份配置跑真实发布。

## 其他

- `coverage.json.bak-*`：人工/脚本改账本前的备份，可保留最近几份。
- `startup.log`：旧版 Windows GUI 遗留的大日志（当前 Rust GUI 不写），可安全删除。
