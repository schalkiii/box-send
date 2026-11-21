# box-send 交付前审计报告（2026-11-21）

## 1. 现状

- 代码规模：Rust workspace（`bsend` 库 + `bsend-cli` + `bsend-gui`），另有 Swift/Windows 历史实现；
  当前自动化执行路径以 Rust CLI 为准。
- 发布路径：`run_reseed` 负责源种下载、目标站预审、上传、目标 torrent 回填与 qB 添加；
  `autopost` 负责队列、门禁与日限额。
- 当前运行态：仅 luckpt 开启真实发布；`publishEnabled=true`、`skipPostCheck=true`、
  `skipHashCheck=true`，白名单仅 luckpt。
- qB 版本：5.3.0（WebAPI 2.16）。该版本已移除 `skip_checking`，实际使用 `seedMode=true`。

## 2. 问题清单

| # | 位置 | 问题 | 严重度 | 影响 |
|---|------|------|--------|------|
| 1 | `reseed.rs` / 历史草稿 | 人工草稿曾绕过源种身份校验，发布本地没有的 1080p 版本（绿里奇迹 id 61299） | 高 | 站点污染、假辅种、需人工删除 |
| 2 | `qbittorrent.rs` | 依赖 qB 旧参数 `skip_checking`；qB 5.3 忽略该参数 | 高 | 每条新种进入 hash 校验，错过站点出种时效 |
| 3 | `reseed.rs` | 判重失败（网络异常/预算耗尽）曾继续上传 | 高 | 站点已有同片时仍可能产生重复发布 |
| 4 | `coverage.rs` / 候选生成 | 外部 Python 脚本按标题/IMDb 预筛，脚本不在仓库、无回归测试 | 中 | 候选筛选行为不可重复验证 |
| 5 | `reseed.rs` | 媒介/编码/音轨/标签靠启发式词表，真实站表单有大量变体 | 中 | 预审拦截率高，需要反复补词元 |
| 6 | 文档 | `SITE-REGISTER` 的 55/3/34 门禁统计早于本轮发布硬化；README 尚未说明 qB 5.3 seedMode | 中 | 交接时状态误导 |
| 7 | 测试 | 无 qB HTTP 添加参数、判重失败路径、源分片不一致路径的自动化回归测试 | 高 | 关键事故只能靠真实发种发现 |

## 3. 改进建议

1. **（已完成，需回归）源种身份硬闸**：`reseed.rs` 的 `matching_local_member` 要求源种 pieces hash
   与 qB 中 100% 成员一致；单测待补纯函数测试。
2. **（已完成，需回归）qB 5.3**：`qbittorrent.rs` 同时发送 `skip_checking` 与 `seedMode`；
   建议增加请求体单测（可拆出无网络 form builder）。
3. **（已完成）判重 fail-closed**：IMDb/标题检索异常或预算耗尽即拒绝上传；建议把
   “明确 Miss 才放行”写成状态机测试。
4. **待做**：把候选筛选脚本纳入 `scripts/`，不依赖临时目录；脚本只读、无 token 输出。
5. **待做**：为 `build_upload_fields` 增加真实表单 fixtures（AC3、DD+、H265、粤语、中字、
   REMUX、死图剪除）回归测试。
6. **待做**：更新 `SITE-REGISTER` 的当前门禁/真实发布统计，README 增加 qB 5.3 语义。
7. **待做**：删除绿里奇迹站点条目（id 61299）需用户站内操作；本地 qB 已无该种。

## 4. 优先级与并行性

- P0 串行：qB seedMode 回归 + 源分片硬闸回归 + 判重 fail-closed 回归（均触及 `reseed.rs` / `qbittorrent.rs`）。
- P1 可并行：候选脚本入库（`scripts/`，不影响 Rust 源）；文档同步（`doc/`、`README.md`）。
- P2 串行：真实发种批量回归依赖上述 P0 完成。

## 5. 测试分支执行结果（software-improvement / testing）

- 基线：151 单测 + 17 e2e 全绿。
- 新增回归：
  - `add_form_uses_seed_mode_for_skip_checking`：断言 qB 5.3 必发 `seedMode=true`。
  - `audio_codec_falls_back_to_mediainfo`：标题无音轨码时从 MediaInfo 选 AC-3。
  - `ddplus_maps_to_ddp_eac3`：`DD+` 归一化为 `dd` 后映射 `DDP/E-AC3`。
  - AC3→DD/AC3、H265→H.265/HEVC、粤语/中字识别用例。
- 结果：154 单测 + 17 e2e 全绿。
- 覆盖率：`cargo llvm-cov` 在当前 Windows GNU stable 因缺少 `profiler_builtins` 无法执行，
  是工具链限制，不是测试失败；覆盖率数字暂标 pending，不用假数据。

## 6. 真实发种回归结果（仅 luckpt）

| 内容 | id | 结果 |
|---|---|---|
| 如父如子 | 61298 | 发布成功；qB `418d95ad610f` 100% |
| 雄狮少年 | 61307 | 发布成功；qB `b5c5a2158450` 100%（seedMode 验证） |

- 绿里奇迹 `61299` 是错误版本发布（1080p CMCT vs 本地 2160p UHD），已撤出 qB；
  站点条目待删除，索引 `review_pending`。
- 其余批次在发布前被程序化拦截：目标站同片版本判重、官方预审图片/音轨/标签未过，
  或源站下载链接 404；没有再把错误种子发出去。

## 7. 本轮追加回归（15:19-16:00）

- 新增 `HDR10` 标签只从 UHD/2160p 标题推断，不再从 1080p MediaInfo 误判。
- 英语标签只在无中文字幕、无中文标题/副标题时添加，避免“英语与中文内容冲突”。
- 音频回退改为“标题已有音轨码优先，MEDIAINFO 只在标题缺失时兜底”。
- 新增测试后：155 单测 + 17 e2e 全绿。
- 继续尝试：Hotel Mumbai / The Intern 被 luckpt 同片其他版本判重；土拨鼠之日/武状元苏乞儿/
  赌侠/鹿鼎记2 被预审图片/中文标题等拦截；没有错误上传。
- 当前真实成功仍为 2 条：如父如子 61298、雄狮少年 61307。
