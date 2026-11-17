#!/usr/bin/env bash
# 本地彩排 .github/workflows/rust.yml 的全部检查命令。
# 用途：改 CI / 依赖 / workspace 成员后，先在这里跑一遍，别把低阶错误推到 CI 上。
set -euo pipefail

cd "$(dirname "$0")/../rust"

echo "== fmt（所有平台）=="
cargo fmt --check

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    echo "== clippy（Windows：全部 crate，含 GPUI GUI）=="
    cargo clippy --workspace --all-targets -- -D warnings
    echo "== test（Windows：全部 crate）=="
    cargo test --workspace
    ;;
  *)
    # Linux 的 GPUI 传递依赖 xattr 0.2.3 与新版 libc 不兼容（ENOATTR 已移除），
    # 因此 CI 与这里都只检查可移植的核心库 + CLI。
    echo "== clippy（Linux：核心库 + CLI）=="
    cargo clippy --workspace --all-targets --exclude bsend-gui -- -D warnings
    echo "== test（Linux：核心库 + CLI）=="
    cargo test --workspace --exclude bsend-gui
    ;;
esac

echo "== 本地 CI 彩排通过 =="
