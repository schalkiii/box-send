# Changelog

版本以 git tag 为准（Rust workspace 当前 `0.1.0`）。只记录用户可见/行为变化；
逐条技术细节见 `doc/AUDIT-REPORT-*.md` 与 git 历史。

## [Unreleased]

### Added

- 辅种：pieces-hash 批量通道 + 名字检索（严格 / 宽松 / 最大成员种名兜底）。
- 转种：预检 → 官方预审 → 上传 → 发后自检 → 推种校验全流程；`autopost`
  白名单 + 日限额（北京时间）+ 退避 + 审计（`auto-audit.jsonl`）。
- GUI（GPUI）：概览 / 转种草稿 / 自动发种 / 配置 / 诊断 / 帮助。
- 引擎加入下载器的种子统一带 `Boxsend` 标签（已有种子走 `addTags` 追加）。
- Rust release 工作流（`v*` tag 触发，CLI + GUI + SHA256SUMS）。
- 搜索结果多候选择优：同片多版本时优先取名称最接近发布名的条目。

### Changed

- 自动发种审计区分 `posted / skipped_duplicate / other_version / existed_pending / failed`。
- `otherVersion` 纳入终态，不再每 7 天重复入队。
- 限流地板按轮次保留（一次成功不再回到 8 秒节奏）。
- `requireLinks` / `requireMediaInfo` 放宽同时作用于门禁与预检。

### Fixed

- 辅种新加种带限速/异常 note 时不再误落 `assisted` 捷径（会继续校验并启动）。
- 源种预算耗尽不再落 7 天 `miss` 负缓存。
- 审核渲染脱敏 `passkey/token/secure/uid` 等字段值。
- CookieCloud 错误信息不再包含访问 KEY。
- GUI 跑一轮/基线结论落盘（`persist=true`）。
- 跨进程运行锁（`bsend-state.lock`），防 GUI + CLI 并发突破日限额/覆盖账本。
- `autopost --dry-run` 在空白名单时逐条跑门禁演练（此前直接全部跳过，与提示文案不符）。

## [0.1.0] - 2026-11-22

首个 Rust 版可运行版本（引擎 + CLI + GUI）。能力与配置见 `rust/README.md`，
站点门禁/审计结论见 `doc/`。
