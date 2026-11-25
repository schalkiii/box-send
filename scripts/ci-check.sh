#!/usr/bin/env bash
# 本地彩排 .github/workflows/rust.yml 的全部检查命令。
# 用途：改 CI / 依赖 / workspace 成员后，先在这里跑一遍，别把低阶错误推到 CI 上。
set -euo pipefail

cd "$(dirname "$0")/../rust"

echo "== fmt（所有平台）=="
cargo fmt --all -- --check

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    # 本机默认 GNU 工具链缺 -lktmw32；MSVC 没装 link.exe 也链接不了 GUI。
    # CI 的 Windows runner 两样都有，所以 CI 跑全量；本地缺工具链时明确降级，
    # 不把 GUI 的链接错误当成代码错误。设 BSEND_FORCE_FULL=1 可强制跑全量。
    if [[ "${BSEND_FORCE_FULL:-0}" == "1" ]] \
      || { rustc -vV | grep -q 'host: .*windows-msvc' && command -v link.exe >/dev/null 2>&1; }; then
      echo "== clippy（Windows：全部 crate，含 GPUI GUI）=="
      cargo clippy --workspace --all-targets -- -D warnings
      echo "== test（Windows：全部 crate）=="
      cargo test --workspace
    else
      echo "== 本机没有 MSVC link.exe：跳过 bsend-gui，核心库 + CLI 照常跑 =="
      echo "== clippy（Windows：核心库 + CLI）=="
      cargo clippy --workspace --all-targets --exclude bsend-gui -- -D warnings
      echo "== test（Windows：核心库 + CLI）=="
      cargo test --workspace --exclude bsend-gui
    fi
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
