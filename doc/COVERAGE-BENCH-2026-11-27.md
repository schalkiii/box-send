# `coverage.json` 落盘基准（2026-11-27）

## 结论

当前不需要分片或改增量落盘。23.7 MB 索引的 Rust 加载约 **67 ms**、整文件保存约 **64 ms**，一次进程峰值工作集约 **81.2 MB**；两项耗时加起来约 **131 ms**，只要一轮任务超过 2.6 秒，落盘占比就低于审计设定的 5% 阈值。现有单文件 `coverage.json` 保持不动。

## 测法

- 工具：`rust/crates/bsend/examples/coverage-bench.rs`，复制真实 `coverage.json` 到临时目录，只测 `ContentIndex::load` 和 `ContentIndex::save`，不碰原文件。
- 数据：`C:\Users\Schal\AppData\Local\BoxSend\coverage.json`，3,388 段内容。
- 命令：

```powershell
Set-Location rust
cargo run --release -p bsend --example coverage-bench -- "$env:LOCALAPPDATA\BoxSend\coverage.json"
```

- 输出：`src_bytes=23698094 out_bytes=22967447 contents=3388 load_ms=67 save_ms=64`。
- 峰值工作集：PowerShell `Start-Process` 后每 5 ms 采样 `PeakWorkingSet64`，最高 81.2 MB。该值含进程本身和二进制，不是纯 JSON 对象增量。
- 证据：`artifacts/coverage-bench-20261127.txt`。

## 何时再回来分片

出现下面任一情况再改存储格式，不在没测之前动：

1. 索引超过 100 MB，或加载 + 保存超过一轮总时长的 5%；
2. 保存时出现明显卡顿（超过 500 ms）或内存峰值超过 500 MB；
3. 多进程同时写同一份 `coverage.json` 造成冲突。

分片时按 `site` 或 `content key` 前缀切文件，保留旧单文件迁移路径，并补“分片缺失 / 旧文件迁移”用例。
