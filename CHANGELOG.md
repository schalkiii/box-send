# Changelog

版本以 git tag 为准（Rust workspace 当前 `0.1.0`）。只记录用户可见/行为变化；
逐条技术细节见 `doc/ARCHITECTURE.md`、`doc/OPERATIONS.md`、`doc/ROADMAP.md` 与 git 历史。

## [Unreleased]

### Added

- 辅种：pieces-hash 批量通道 + 名字检索（严格 / 宽松 / 最大成员种名兜底）。
- 转种：预检 → 官方预审 → 上传 → 发后自检 → 推种校验全流程；`autopost`
  白名单 + 日限额（北京时间）+ 退避 + 审计（`auto-audit.jsonl`）。
- GUI（GPUI）：概览 / 转种草稿 / 自动发种 / 配置 / 诊断 / 帮助。
- 引擎新加入下载器的种子统一带 `Boxsend` 标签；已存在的同 hash 种子不改动（不补挂 tracker、不改标签）。
- Rust release 工作流（`v*` tag 触发，CLI + GUI + SHA256SUMS）。
- 搜索结果多候选择优：同片多版本时优先取名称最接近发布名的条目。
- `reseed-log.jsonl`：CLI/GUI/autopost 每次转种落一行动作日志，含 `seeded`
  （是否已确认在 qB 做种），便于排查「站点发了、qB 没辅上」。
- `scripts/seeding-audit.ps1` + `doc/OPERATIONS.md`：只读巡检覆盖率异常记录、
  未成种记录与下载器卡种；不打印 cookie/passkey。
- `scripts/site-matrix.ps1` + `doc/SITES.md`：只读生成 147 站适配矩阵
  （启用/暂停/停转种/实测/formcheck/pieces/cookie 数）。
- `formcheck` 落盘记录补充提交地址（`action`）与每个下拉的全量选项表（`selectOptions`）；
  旧字段保持不变，站点矩阵脚本兼容。
- 覆盖报表新增按 qB 统计的“已有 / 缺口 / 覆盖率”：`qbCovered`、`qbMissing`、`qbRatio`；
  `bsend status` 单站表改为这三列，`bsend report` 同时保留旧列作历史对比。
- 自动发种的待发清单改为每轮从 qB 实时种子列表派生：白名单站点没有 tracker、源种子已定位、
  内容还在 qB、没有禁止转载才生成；同名内容对同站只出一条，按体积排序并按每站额度截断。
- 文档收敛（2026-11-23）：17 份 `doc/` 文档合并为 5 份（`ARCHITECTURE` / `OPERATIONS` /
  `SITES` / `SITE-DEBUG` / `ROADMAP`），批次报告与历史实机记录并入后退役；正文改写为白话，
  先讲原因再讲做法，技术字段首次出现时附一句解释。
- 站点配置新增 `overrides.announceURL`：填写该站含个人 passkey 的完整 tracker 地址。
  第一次向 qB 里还没有该站种子的站点发种时，靠它构造只带目标站 tracker 的上传种子；
  不填则继续从 qB 现有 tracker 里找，两边都没有时中止并提示配置。
- 转种链路改为单种子单站点：从 qB 导出源种后字节级改写 tracker、删 `announce-list`、
  写目标站专属 `info.source` 生成新 hash；上传后从目标站下载 `.torrent` 独立挂回 qB，
  不再调用 `addTrackers`，也不把多个站的 announce 合并进同一个种子。

### Changed

- 发布口径（2026-11-22）：目标站已有同名其他版本（分集/全集、不同压制组）时**照样发布**，
  画质高低交给站点规则裁决，工具不再自行比较 2160p/1080p（`assist.allowOtherVersion`）。
- 自动发种新增标题级内容黑名单 `assist.skipKeywords`：教育/成人等命中即跳过，不占上传预算。
  匹配规则：ASCII 关键词按整词命中（`Adult` 不误伤 `Adulthood`），中文关键词按子串；
  名称先归一小写、非字母数字折成空格再比。
- `existed` 判定收紧为结构性证据（`existed=1` 跳转 / `errorbox` 重复措辞），上传表单原样返回
  不再算重复；定位兜底去掉 IMDb 区，未验证的回包 id 不再写进覆盖表 `url`。
- `other_version` 从「待人工」改为明确的「不同内容，可另行发布」；总大小不同不再进人工队列。
- `bsend status` 新增单站视图（已有 / 缺口 / 覆盖率 / 草稿 / 已跳过）与「白名单站点暂无待发」提示；待找源清单按内容名去重。
- 自动发种白名单为空时的口径固定：真实发布不发任何站，避免漏填后全站开火；`--dry-run` 才用全部站点做发布检查预演。

- 自动发种审计区分 `posted / skipped_duplicate / other_version / existed_pending / failed`。
- `otherVersion` 纳入终态，不再每 7 天重复入队。
- 限流地板按轮次保留（一次成功不再回到 8 秒节奏）。
- `requireLinks` / `requireMediaInfo` 放宽同时作用于发布检查与预检。
- 同 hash 目标种一律不改动原种子（不补挂 tracker、不打标签），记「同 hash 跳过」；
  qB 不允许同 hash 两条，新加种子只适用于 hash 不同的目标种。

### Fixed

- 发后定位不再取回包里的第一条详情链接：详情和下载必须按 id 配对，最终跳转是详情页时优先用它；
  详情和下载 id 对不上就留空转 `posted_unresolved`，避免把成功页里的相关旧种（hdvideo 14469）挂进 qB。
  `userdetails.php?id=` 也不再被当成种子详情。
- 辅种新加种带限速/异常 note 时不再误落 `assisted` 捷径（会继续校验并启动）。
- 源种预算耗尽不再落 7 天 `miss` 负缓存。
- 审核渲染脱敏 `passkey/token/secure/uid` 等字段值。
- CookieCloud 错误信息不再包含访问 KEY。
- GUI 跑一轮/基线结论落盘（`persist=true`）。
- 跨进程运行锁（`bsend-state.lock`），防 GUI + CLI 并发突破日限额/覆盖记录。
- `autopost --dry-run` 在空白名单时逐条跑发布检查演练（此前直接全部跳过，与提示文案不符）；
  `--limit` 在演练下限制审阅条数（此前不生效，会跑完全部草稿）。
- 站内判重命中（同分片）/ 上传回包 existed 时，下载目标种并推入下载器做种
  （此前只记「已覆盖/待确认」，qB 里没有目标站 tracker）。
- GUI 双击启动：配置文件默认回退到 `%LOCALAPPDATA%\BoxSend\boxsend.json`；
  加载失败弹可见错误框（此前无控制台，双击后静默退出）。
- **自动发种改为「审阅产物直发」**（BUG-M7）：发布检查通过后直接把 `ReviewReport`
  交给 `run_reseed_reviewed`，提交字段与发布检查结果同一份；不再二次抓源页/上传页、
  不重复调官方预审，消除「审阅字段 ≠ 真正上传字段」的漂移。单发 CLI/GUI 仍走现场构造。
- 新增站点写接口级 e2e：审阅产物直发（提交体字段一致、源页/表单零重抓）与
  `takeupload` 拒收（保留 errorbox 原文、不推种、不改覆盖记录）。
- 上传提交地址防误配：action 只在上传表单块内取；命中搜索/奖励/登录/详情等非上传端点时
  中止提交，不再 POST 到错误接口（站外绝对提交地址照常放行）。
- 部分站点提交后不发跳转：回包正文带唯一新种详情页链接时识别为发布成功；表单重填页
  （multipart）或正文挂多个详情链接不算成功。

## [0.1.0] - 2026-11-22

首个 Rust 版可运行版本（引擎 + CLI + GUI）。能力与配置见 `rust/README.md`，
站点规则/审计结论见 `doc/`。
