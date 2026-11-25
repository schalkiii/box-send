#!/usr/bin/env python3
"""把简介里的图片逐张下载并转存到 pixhost，输出替换后的简介。

只改图片链接，不动排版。任何一张下载或转存失败都不写输出文件，
避免留下半套图又发出去。

用法：
  py -3 scripts/rehost-descr-images.py --descr-file in.txt --out-file out.txt
  py -3 scripts/rehost-descr-images.py --descr-file in.txt --out-file out.txt --apply-check
"""

from __future__ import annotations

import argparse
import io
import re
import sys
import tempfile
from pathlib import Path
from urllib.parse import urlparse

import requests

UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/124.0 Safari/537.36"
)

BB = re.compile(r"(\[img(?:=[^\]]*)?\])([^\[\]]+)(\[/img\])", re.I)
HTML = re.compile(r'(<img\b[^>]*?\bsrc=")([^"]+)(")', re.I)


def images(text: str) -> list[str]:
    found = [m.group(2) for m in BB.finditer(text)]
    found += [m.group(2) for m in HTML.finditer(text)]
    seen, out = set(), []
    for u in found:
        u = u.strip()
        if u.startswith("http") and u not in seen:
            seen.add(u)
            out.append(u)
    return out


def fetch(url: str, referer: str) -> bytes:
    headers = {"User-Agent": UA}
    if referer:
        headers["Referer"] = referer
    r = requests.get(url, headers=headers, timeout=60)
    r.raise_for_status()
    ct = (r.headers.get("Content-Type") or "").lower()
    if ct and not (ct.startswith("image/") or ct == "application/octet-stream"):
        raise RuntimeError(f"不是图片：{ct}")
    if len(r.content) < 1024:
        raise RuntimeError(f"图片太小：{len(r.content)} 字节")
    return r.content


def upload_pixhost(data: bytes, name: str) -> str:
    r = requests.post(
        "https://api.pixhost.to/images",
        data={"content_type": "0", "max_th_size": "420"},
        files={"img": (name, io.BytesIO(data), "image/jpeg")},
        headers={"User-Agent": UA, "Accept": "application/json"},
        timeout=120,
    )
    r.raise_for_status()
    show = (r.json() or {}).get("show_url") or ""
    if not show:
        raise RuntimeError(f"pixhost 返回没有 show_url：{r.text[:200]}")
    return re.sub(r"https://pixhost\.to/show/", "https://img3.pixhost.to/images/", show)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--descr-file", required=True)
    ap.add_argument("--out-file", required=True)
    ap.add_argument("--referer", default="")
    args = ap.parse_args()

    text = Path(args.descr_file).read_text(encoding="utf-8")
    urls = images(text)
    if not urls:
        print("简介里没有图片")
        return 2

    mapping: dict[str, str] = {}
    with tempfile.TemporaryDirectory() as td:
        for i, url in enumerate(urls, 1):
            if "pixhost.to/" in url:
                print(f"[{i}/{len(urls)}] 已是 pixhost，跳过 {url}")
                continue
            print(f"[{i}/{len(urls)}] 下载 {url}")
            data = fetch(url, args.referer)
            local = Path(td) / f"{i}.jpg"
            local.write_bytes(data)
            print(f"        {len(data)} 字节，转存 pixhost")
            new = upload_pixhost(data, f"descr-{i}.jpg")
            mapping[url] = new
            print(f"        -> {new}")

    out = text
    for old, new in mapping.items():
        out = out.replace(old, new)
    left = [u for u in images(out) if "pixhost.to/" not in u]
    Path(args.out_file).write_text(out, encoding="utf-8", newline="")
    print(f"替换 {len(mapping)} 张；剩余非 pixhost {len(left)} 张 -> {args.out_file}")
    for u in left:
        print(f"  残留 {u}")
    return 0 if not left else 1


if __name__ == "__main__":
    sys.exit(main())
