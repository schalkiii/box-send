#!/usr/bin/env python3
"""把已发布种子简介里的图片按出现顺序换成新直链（原文只动链接，不动排版）。

用法：
  # 取原简介
  py -3 scripts/fetch-descr.py ubits 379266
  # 按顺序换 1 张海报 + 8 张截图（BBCode 站）
  py -3 scripts/swap-descr-images.py --descr-file "%TEMP%\\bsend-review\\ubits-379266-descr.txt" ^
      --urls "https://images4.imgbox.com/...,https://images4.imgbox.com/,..." --out new.txt
  # 演练 + 提交
  py -3 scripts/edit-seed.py --site ubits --id 379266 --descr-file new.txt
  py -3 scripts/edit-seed.py --site ubits --id 379266 --descr-file new.txt --apply

BBCode（`[img]…[/img]`）与 HTML（`<img src="…">`）都支持；张数对不上就报错不写文件，
避免把海报和截图的顺序串错。
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

BB = re.compile(r"(\[img\])([^\[\]]+)(\[/img\])", re.I)
HTML = re.compile(r'(<img\b[^>]*?\bsrc=")([^"]+)(")', re.I)


def targets(text: str) -> tuple[str, list[re.Match[str]]]:
    hits = list(HTML.finditer(text))
    if hits:
        return "HTML", hits
    hits = list(BB.finditer(text))
    return "BBCode", hits


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--descr-file", required=True)
    ap.add_argument("--urls", required=True, help="新图直链，按出现顺序用逗号分隔")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    text = pathlib.Path(args.descr_file).read_text(encoding="utf-8")
    kind, hits = targets(text)
    urls = [u.strip() for u in args.urls.split(",") if u.strip()]
    print(f"简介格式 {kind}，图 {len(hits)} 张；新链接 {len(urls)} 条")
    for i, m in enumerate(hits, 1):
        print(f"  {i}. 旧: {m.group(2)[:90]}")
    if len(hits) != len(urls):
        print("张数对不上，不写文件；先按原简介的实际张数准备图片")
        return 2

    pieces, pos = [], 0
    for m, url in zip(hits, urls):
        pieces.append(text[pos:m.start()])
        pieces.append(m.group(1) + url + m.group(3))
        pos = m.end()
    pieces.append(text[pos:])
    out = "".join(pieces)

    pathlib.Path(args.out).write_text(out, encoding="utf-8", newline="")
    left = [u for u in re.findall(r'src="([^"]+)"', out) + re.findall(r"\[img\]([^\[\]]+)", out, re.I) if "imgbox" not in u]
    print(f"替换 {len(urls)} 处；非 imgbox 残留 {len(left)} 处")
    print(f"新简介 {len(out)} 字符 -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
