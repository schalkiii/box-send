#!/usr/bin/env python3
r"""把一个站点的编辑页表单读出来，把简介字段原样存到文件，方便改图后回填。

用法：py -3 fetch-descr.py ubits 379266
输出：%TEMP%\bsend-review\<site>-<id>-descr.txt 与同目录的 fields.txt（字段名与长度）
"""

from __future__ import annotations

import importlib.util
import json
import os
import re
import sys
from pathlib import Path
from urllib.parse import urlparse

import requests

BASE = Path(os.environ["LOCALAPPDATA"]) / "BoxSend"
REPO = Path(__file__).resolve().parent
FIX_SCRIPT = REPO / "fix-bbcode-descr.py"
OUT = Path(os.environ["TEMP"]) / "bsend-review"


def load_form_parser():
    spec = importlib.util.spec_from_file_location("boxsend_fix_descr", FIX_SCRIPT)
    mod = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(mod)
    return mod


def main() -> int:
    site, sid = sys.argv[1], sys.argv[2]
    cfg = json.loads((BASE / "boxsend.json").read_text(encoding="utf-8"))
    base_url = next(s["url"].rstrip("/") for s in cfg["sourceSites"] if s["id"] == site)
    host = urlparse(base_url).hostname or ""
    cookies = json.loads((BASE / "cookies.json").read_text(encoding="utf-8"))
    s = requests.Session()
    s.headers.update({"User-Agent": "Mozilla/5.0"})
    for c in cookies.get(host, []):
        s.cookies.set(c["name"], c["value"], domain=host)
    detail = f"{base_url}/details.php?id={sid}"
    edit = f"{base_url}/edit.php?id={sid}"
    r = s.get(edit, headers={"Referer": detail}, timeout=30)
    print(f"编辑页 HTTP {r.status_code} len={len(r.text)}")
    mod = load_form_parser()
    form = mod.FormParser()
    form.feed(r.text)
    if not form.found:
        print("没有 takeedit 表单")
        return 1
    fields = {k: (len(v) if isinstance(v, str) else v) for k, v in form.data.items()}
    (OUT / f"{site}-{sid}-fields.txt").write_text(
        "\n".join(f"{k}\t{v}" for k, v in fields.items()), encoding="utf-8"
    )
    descr_key = next((k for k in ("descr", "release_desc", "description", "content") if k in form.data), None)
    if not descr_key:
        print("没有简介字段")
        return 1
    raw = form.data[descr_key]
    (OUT / f"{site}-{sid}-descr.txt").write_text(raw, encoding="utf-8")
    print(f"简介字段 {descr_key}，长度 {len(raw)}")
    print("含 BBCode [img]:", bool(re.search(r"\[img", raw, re.I)), "含 <img:", "<img" in raw.lower())
    print("贴图库张数:", len(re.findall(r"tietuku", raw)))
    # 预览前 160 字，换行压成空格
    print("开头:", re.sub(r"\s+", " ", raw[:160]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
