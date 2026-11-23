#!/usr/bin/env python3
"""编辑 NexusPHP 已发布种子的标题/简介（只改表单，不动种子文件）。

用法：
  py -3 scripts/edit-seed.py --site cangbao --id 43994 --descr-file new.txt
  py -3 scripts/edit-seed.py --site panda --id 141276 --name "English Title" --descr-file new.txt --apply

默认只读演练；`--apply` 才 POST takeedit.php。表单里除被指定的字段外，
其余 input/textarea/select 原样回填，避免误改分类/媒介/标签。
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import sys
from pathlib import Path
from urllib.parse import urljoin, urlparse

import requests

BASE = Path(os.environ["LOCALAPPDATA"]) / "BoxSend"
FIX_SCRIPT = Path(__file__).with_name("fix-bbcode-descr.py")
DESCR_FIELDS = ("descr", "release_desc", "description", "content")


def load_form_parser():
    spec = importlib.util.spec_from_file_location("boxsend_fix_descr", FIX_SCRIPT)
    mod = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(mod)
    return mod


def find_site_url(site: str) -> str:
    cfg = json.loads((BASE / "boxsend.json").read_text(encoding="utf-8"))
    for s in cfg.get("sourceSites", []):
        if s.get("id") == site:
            return s["url"].rstrip("/")
    raise SystemExit(f"配置里找不到站点 {site}")


def session_for(host: str) -> requests.Session:
    cookies = json.loads((BASE / "cookies.json").read_text(encoding="utf-8"))
    s = requests.Session()
    s.headers.update({"User-Agent": "Mozilla/5.0"})
    for c in cookies.get(host, []):
        s.cookies.set(c["name"], c["value"], domain=host)
    return s


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--site", required=True)
    ap.add_argument("--id", required=True)
    ap.add_argument("--descr-file")
    ap.add_argument("--name")
    ap.add_argument("--subtitle")
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()

    base_url = find_site_url(args.site)
    detail_url = f"{base_url}/details.php?id={args.id}"
    edit_url = f"{base_url}/edit.php?id={args.id}"
    host = urlparse(base_url).hostname or ""
    s = session_for(host)
    r = s.get(edit_url, headers={"Referer": detail_url}, timeout=30)
    print(f"编辑页 HTTP {r.status_code} len={len(r.text)}")
    if r.status_code != 200:
        return 1

    mod = load_form_parser()
    form = mod.FormParser()
    form.feed(r.text)
    if not form.found:
        print("没有 takeedit 表单（可能无编辑权限）")
        return 1
    data = dict(form.data)
    if form.tags:
        data["tags[4][]"] = form.tags

    changed = []
    if args.name is not None:
        data["name"] = args.name
        changed.append("name")
    if args.subtitle is not None:
        data["small_descr"] = args.subtitle
        changed.append("small_descr")
    if args.descr_file:
        new_descr = Path(args.descr_file).read_text(encoding="utf-8")
        field = next((f for f in DESCR_FIELDS if f in data), None)
        if not field:
            print("没有简介字段")
            return 1
        data[field] = new_descr
        changed.append(field)

    print("将修改：" + ", ".join(changed))
    print(f"简介长度：{len(data.get('descr', ''))} 字节")
    if not args.apply:
        print("只读演练完成；加 --apply 才提交")
        return 0

    resp = s.post(
        urljoin(edit_url, "takeedit.php"),
        data=data,
        headers={"Referer": edit_url, "Origin": base_url},
        allow_redirects=False,
        timeout=30,
    )
    debug_dir = BASE / "upload-debug" / "edit-seed"
    debug_dir.mkdir(parents=True, exist_ok=True)
    raw_path = debug_dir / f"{args.site}-{args.id}-response.html"
    raw_path.write_text(resp.text, encoding="utf-8")
    print(f"takeedit HTTP {resp.status_code} location={resp.headers.get('Location') or '-'} body={len(resp.text)} saved={raw_path}")
    if resp.status_code not in (301, 302, 303):
        # 有的站返回 200 + 空回包也算成功；若正文含明确错误则判失败
        for bad in ("错误", "失败", "无权", "不能", "拒绝", "Invalid", "Error"):
            if bad in resp.text:
                print(f"takeedit 回包含失败提示「{bad}」，请人工核对 {raw_path}")
                return 1
        print("takeedit 未返回跳转，按站点 200 空回包处理；请人工核对详情页")
        return 0
    follow = urljoin(edit_url, resp.headers.get("Location", detail_url))
    detail = s.get(follow, headers={"Referer": edit_url}, timeout=30)
    print(f"详情页 HTTP {detail.status_code} len={len(detail.text)}")
    if detail.status_code != 200:
        return 1
    # 常见失败文案
    for bad in ("错误", "失败", "无权", "不能", "拒绝"):
        if f">{bad}<" in detail.text or f">{bad} " in detail.text:
            print(f"详情页含失败提示「{bad}」，请人工核对")
            return 1
    print("提交完成；详情页已可访问")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
