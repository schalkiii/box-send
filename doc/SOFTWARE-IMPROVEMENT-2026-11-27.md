# Software Improvement 修订摘要（2026-11-27）

本轮只走**代码质量**分支，不做功能改动、不混架构重构。

## 一、基线

- 动手前 `cargo test --workspace` 535 条全绿，`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings` 通过。
- 代码基线 `0f1ff35`（新发种收尾语义已修）。

## 二、改动点

| 位置 | 改动 | 原因 |
|---|---|---|
| `rust/crates/bsend/src/coverage.rs` | 覆盖记录缓存与待查队列的锁改用 `lock_unpoisoned` | 一处 panic 不再让整轮任务连锁失败 |
| `coverage.rs` | 分页预算裁剪改用 `let-else`，去掉 `max_by_key().unwrap()` | 避免空队列时的隐式 panic |
| `rust/crates/bsend/src/reseed.rs` | UTF-8 取字符改 `let-else`；标签栈弹出改 `let-else` | 边界条件不再直接 `unwrap` |
| `rust/crates/bsend/src/sites/nexusphp/detail.rs` | 详情链接前字符判断改 `Option::is_none_or`；三处固定选择器补 `expect` 文案 | 去掉边界 `unwrap`，固定选择器失败时能定位 |
| `rust/crates/bsend/src/sites/nexusphp/upload.rs` | 保留原有 `expect` 文案，不改语义 | 重试循环必然记录过错误，属不变量 |
| `rust/crates/bsend/src/coverage.rs` | 修 clippy 的 `needless_borrow` | 保持 `-D warnings` 零告警 |

## 三、逐点验证

- 改 `coverage.rs` 后跑 `cargo test -p bsend --lib coverage::`：28 条通过。
- 改 `reseed.rs` 后跑 `cargo test -p bsend --lib reseed::`：168 条通过。
- 全量 `cargo test --workspace`：535 条全绿（`artifacts/workspace-full-test-20261127-final29.log`）。
- `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings`：通过（`artifacts/fmt-check-20261127-final28.log`、`artifacts/clippy-20261127-final28.log`）。
- 真实运行：新 release 跑 `bsend status`，覆盖分布从“已辅种 28 / 已转种 116”变为“已辅种 27 / 已转种 117”，与 hdvideo 79168 人工纠正后的语义一致（`artifacts/status-20261127-final30.txt`，最终 release 为 `build-release-20261127-final30.log`）。

## 四、未做与理由

- 剩余生产路径 `unwrap/expect` 10 处未一次性清完：`index.rs:385`、`report.rs:172`、`reseed/upload.rs:507`、`detail.rs` 三处固定选择器、`sites/nexusphp/upload.rs:40`、`commands.rs:521`、`bsend-gui/src/main.rs:361` 与 `:673`。这些都有前置不变量；逐处改要单独验证，避免把“不会发生”改成“静默吞错”。已列入 r3 审计 S5。
- 没有做架构重构（`coverage.rs`/`commands.rs` 再拆）和功能改动；qB 50,000 条分页是功能/可靠性改动，列入 r3 S1，不混进代码质量分支。
- 后续补丁：S1 qB 分页已在同日晚单独完成，当前全量 536 条、fmt/clippy、真实 `bsend status` 42,470 条全部通过；本报告的 535 条是代码质量分支交付时的基线。

## 五、完成判据

- [x] 改动只限锁恢复、边界判断、断言文案、格式与注释，无功能语义变化
- [x] 每个改动点立即跑对应模块测试
- [x] 全量 535 条、fmt、clippy 通过
- [x] 真实 `bsend status` 验证计数变化
- [x] 剩余项列清单，未擅自扩大改动
