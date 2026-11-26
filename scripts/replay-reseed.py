#!/usr/bin/env python3
"""Run one reseed draft in dry-run mode and assert the expected output.

This is a guard for fixes that were first found on a live seed.  The command
never submits anything: it calls `bsend reseed --draft <N>` without `--yes`.
Use `--expect` for text that must appear (for example a selected field or an
image count) and `--reject` for a regression marker that must not appear.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_EXE = REPO_ROOT / "rust" / "target" / "release" / "bsend.exe"
DEFAULT_CONFIG = Path(os.environ.get("LOCALAPPDATA", "")) / "BoxSend" / "boxsend.json"

DEFAULT_SHOW = (
    "目标站",
    "提交地址",
    "注入本机视频截图",
    "简介剪掉",
    "预检",
    "将提交的字段",
    "source_sel",
    "processing_sel",
    "team_sel",
    "阻断",
    "失败",
    "错误",
)


def _print_relevant(output: str, needles: tuple[str, ...]) -> None:
    lines = output.splitlines()
    for line in lines:
        if any(needle in line for needle in needles):
            print(line)


def main() -> int:
    parser = argparse.ArgumentParser(description="Replay a reseed draft without submitting")
    parser.add_argument("--draft", type=int, required=True)
    parser.add_argument("--expect", action="append", default=[], metavar="TEXT",
                        help="substring that must appear in the output (repeatable)")
    parser.add_argument("--reject", action="append", default=[], metavar="TEXT",
                        help="substring that must not appear in the output (repeatable)")
    parser.add_argument("--exe", default=str(DEFAULT_EXE))
    parser.add_argument("--config", default=str(DEFAULT_CONFIG))
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--show", action="append", default=None,
                        help="extra line filter; repeatable (default is a short summary filter)")
    parser.add_argument("--full", action="store_true", help="print the full bsend output")
    args = parser.parse_args()

    exe = Path(args.exe)
    config = Path(args.config)
    if not exe.is_file():
        print(f"找不到 bsend：{exe}", file=sys.stderr)
        return 2
    if not config.is_file():
        print(f"找不到配置：{config}", file=sys.stderr)
        return 2

    command = [str(exe), "--config", str(config), "reseed", "--draft", str(args.draft)]
    print("[replay] " + " ".join(command))
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=args.timeout,
            check=False,
        )
    except subprocess.TimeoutExpired:
        print(f"[replay] 超时（{args.timeout}s）", file=sys.stderr)
        return 2

    output = completed.stdout + "\n" + completed.stderr
    if args.full:
        print(output)
    else:
        needles = tuple(args.show) if args.show else DEFAULT_SHOW
        _print_relevant(output, needles)

    if completed.returncode != 0:
        print(f"[replay] bsend 退出码 {completed.returncode}", file=sys.stderr)
        return 1

    missing = [text for text in args.expect if text not in output]
    unexpected = [text for text in args.reject if text in output]
    if missing:
        print("[replay] 缺少预期内容：", file=sys.stderr)
        for text in missing:
            print(f"  - {text}", file=sys.stderr)
    if unexpected:
        print("[replay] 出现回归标记：", file=sys.stderr)
        for text in unexpected:
            print(f"  - {text}", file=sys.stderr)

    if missing or unexpected:
        print("[replay] 失败", file=sys.stderr)
        return 1

    print(f"[replay] 通过（draft {args.draft}，{len(args.expect)} 条预期）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
