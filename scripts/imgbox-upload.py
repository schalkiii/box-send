#!/usr/bin/env python3
"""把本地图片上传到 imgbox.com，返回可直接嵌进 PT 简介的原图直链。

为什么选 imgbox：
  - 匿名可用，不需要注册/API key；
  - 单图 10MB 上限，直链无热链限制（实测带 cangbao/panda Referer 均 200）；
  - 图床页面公开、无到期时间；不保证永久，重要资源仍应自建/备份。

用法：
  py -3 scripts/imgbox-upload.py poster.jpg screenshot1.jpg
  py -3 scripts/imgbox-upload.py --json images/*.jpg

输出 JSON：
  {"ok": true, "gallery_edit": "...", "files": [{"name": "...", "url": "...", "page": "..."}]}
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import requests

BASE = "https://imgbox.com"
UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) "
    "Gecko/20100101 Firefox/132.0"
)


def create_session() -> tuple[requests.Session, str]:
    s = requests.Session()
    s.headers.update({"User-Agent": UA})
    r = s.get(BASE, timeout=30)
    r.raise_for_status()
    m = re.search(
        r'input\s+name="authenticity_token"\s+type="hidden"\s+value="([^"]+)"',
        r.text,
    )
    if not m:
        raise RuntimeError("imgbox 首页没有 authenticity_token（页面结构变了？）")
    return s, m.group(1)


def create_gallery(s: requests.Session, csrf: str) -> dict:
    r = s.post(
        BASE + "/ajax/token/generate",
        headers={
            "X-CSRF-Token": csrf,
            "X-Requested-With": "XMLHttpRequest",
            "Content-Type": "application/x-www-form-urlencoded; charset=UTF-8",
            "Origin": BASE,
            "Referer": BASE + "/",
        },
        data={
            "gallery": "true",
            "gallery_title": "pt-desc",
            "comments_enabled": "0",
        },
        timeout=30,
    )
    r.raise_for_status()
    data = r.json()
    if not data.get("token_id") or not data.get("token_secret"):
        raise RuntimeError(f"imgbox token/generate 返回异常：{data}")
    return data


def upload(s: requests.Session, csrf: str, gallery: dict, paths: list[Path]) -> dict:
    files = []
    handles = []
    try:
        for p in paths:
            fh = p.open("rb")
            handles.append(fh)
            files.append(("files[]", (p.name, fh, "application/octet-stream")))
        r = s.post(
            BASE + "/upload/process",
            headers={
                "X-CSRF-Token": csrf,
                "X-Requested-With": "XMLHttpRequest",
                "Origin": BASE,
                "Referer": BASE + "/",
                "Accept": "application/json, text/javascript, */*; q=0.01",
            },
            data={
                "token_id": str(gallery["token_id"]),
                "token_secret": gallery["token_secret"],
                "content_type": "1",
                "thumbnail_size": "100c",
                "gallery_id": gallery["gallery_id"],
                "gallery_secret": gallery["gallery_secret"],
                "comments_enabled": "0",
            },
            files=files,
            timeout=120,
        )
    finally:
        for fh in handles:
            fh.close()
    r.raise_for_status()
    data = r.json()
    if not data.get("files"):
        raise RuntimeError(f"imgbox 上传失败：{data}")
    return data


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+", help="本地图片路径")
    ap.add_argument("--json", action="store_true", help="只输出 JSON，便于脚本消费")
    args = ap.parse_args()
    paths = [Path(p) for p in args.files]
    missing = [str(p) for p in paths if not p.is_file()]
    if missing:
        print("文件不存在：" + ", ".join(missing), file=sys.stderr)
        return 2
    try:
        s, csrf = create_session()
        gallery = create_gallery(s, csrf)
        data = upload(s, csrf, gallery, paths)
    except Exception as exc:
        print(f"上传失败：{exc}", file=sys.stderr)
        return 1
    out = {
        "ok": True,
        "gallery_edit": f"{BASE}/gallery/edit/{gallery['gallery_id']}/{gallery['gallery_secret']}",
        "files": [
            {
                "name": f.get("name"),
                "url": f.get("original_url"),
                "page": f.get("url"),
            }
            for f in data.get("files", [])
        ],
    }
    print(json.dumps(out, ensure_ascii=False))
    if not args.json:
        for f in out["files"]:
            print(f"{f['name']}\t{f['url']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
