#!/usr/bin/env python3
"""编辑 NexusPHP 已发布种子的标题/简介/技术信息/标签（只改表单，不动种子文件）。

用法：
  py -3 scripts/edit-seed.py --site cangbao --id 43994 --descr-file new.txt
  py -3 scripts/edit-seed.py --site panda --id 141276 --name "English Title" --descr-file new.txt --apply
  py -3 scripts/edit-seed.py --site qingwa --id 208692 --technical-info-file bdinfo.txt --audio-codec 7 --apply
  py -3 scripts/edit-seed.py --site qingwa --id 208575 --add-tag 8 --apply
  py -3 scripts/edit-seed.py --site lajidui --id 85084 --processing 11 --source 7 --team 13 --append-image https://img3.pixhost.to/images/7476/815956238_m_36d02007e802163c-1.jpg --apply

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
import time
from pathlib import Path
from urllib.parse import unquote, urljoin, urlparse

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


def cookies_for(cookies: dict, host: str) -> list[dict]:
    """按 host 取 cookie：host 等于键，或是键的子域（与 bsend config.rs 的 cookie_header_for 语义一致）。"""
    h = host.lower()
    out: list[dict] = []
    for key, arr in cookies.items():
        k = key.lower().lstrip(".")
        if h == k or h.endswith("." + k):
            out.extend(arr)
    return out


def session_for(host: str) -> requests.Session:
    cookies = json.loads((BASE / "cookies.json").read_text(encoding="utf-8"))
    s = requests.Session()
    s.headers.update({"User-Agent": "Mozilla/5.0"})
    now = time.time()
    for c in cookies_for(cookies, host):
        exp = c.get("expirationDate")
        if exp is not None and exp > 0 and exp < now:
            continue
        if not c.get("value") and exp is None:
            continue
        s.cookies.set(c["name"], unquote(c.get("value", "")), domain=host)
    return s


def one_field(data: dict[str, str], prefix: str) -> str | None:
    """找前缀字段，优先 [4]（NexusPHP 影视分类）；找不到返回 None。"""
    exact = [k for k in data if k == prefix or k.startswith(prefix + "[")]
    if not exact:
        return None
    preferred = f"{prefix}[4]"
    return preferred if preferred in exact else exact[0]


def option_exists(page: str, field: str, value: str) -> bool:
    """确认下拉里真有这个 value，防把无效值写进表单。"""
    for block in re.findall(
        rf'<select[^>]*name="{re.escape(field)}"[^>]*>.*?</select>', page, re.I | re.S
    ):
        if re.search(rf'<option[^>]*value="{re.escape(str(value))}"', block, re.I):
            return True
    return False


def find_tag_field(page: str) -> str | None:
    """不同站标签字段名不一样（tags[] / tags[4][]），按实际表单字段写。"""
    for tag in re.findall(r"<input[^>]*>", page, re.I):
        m = re.search(r'name=["\']([^"\']*tags[^"\']*)["\']', tag, re.I)
        if m:
            return m.group(1)
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--site", required=True)
    ap.add_argument("--id", required=True)
    ap.add_argument("--descr-file")
    ap.add_argument(
        "--technical-info-file",
        "--bdinfo-file",
        dest="technical_info_file",
        help="写入 technical_info（站点显示为 MediaInfo/BDInfo 栏）；--bdinfo-file 是同一字段的别名",
    )
    ap.add_argument("--clear-technical-info", action="store_true", help="清空 technical_info 字段（有些站不收 MediaInfo）")
    ap.add_argument("--add-tag", action="append", default=[], metavar="VALUE", help="勾选一个标签值，可重复")
    ap.add_argument("--remove-tag", action="append", default=[], metavar="VALUE", help="取消一个标签值，可重复")
    ap.add_argument("--audio-codec", help="audiocodec_sel[4] 的选项值，例如 qingwa 的 Other=7")
    ap.add_argument("--source", help="source_sel[4] 的选项值")
    ap.add_argument("--processing", help="processing_sel[4] 的选项值，例如 lajidui 的 MP4=11")
    ap.add_argument("--team", help="team_sel[4] 的选项值，例如 lajidui 的 Other=5")
    ap.add_argument("--codec", help="codec_sel[4] 的选项值")
    ap.add_argument("--standard", help="standard_sel[4] 的选项值")
    ap.add_argument("--append-image", action="append", default=[], metavar="URL", help="把图片以 [img] 追加到简介末尾，可重复")
    ap.add_argument("--name")
    ap.add_argument("--subtitle")
    ap.add_argument(
        "--extra-field",
        action="append",
        default=[],
        metavar="NAME=VALUE",
        help="透传一个表单字段（站点 JS 异步注入、静态解析取不到的必填项用），可重复",
    )
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()

    base_url = find_site_url(args.site)
    detail_url = f"{base_url}/details.php?id={args.id}"
    edit_url = f"{base_url}/edit.php?id={args.id}"
    host = urlparse(base_url).hostname or ""
    s = session_for(host)
    r = None
    for attempt in range(6):
        try:
            r = s.get(edit_url, headers={"Referer": detail_url}, timeout=25)
            print(f"编辑页 HTTP {r.status_code} len={len(r.text)}")
            if r.status_code == 200:
                break
        except Exception as e:
            print(f"编辑页读取失败（第 {attempt + 1} 次）：{type(e).__name__}: {str(e)[:100]}")
            time.sleep(4)
    if r is None or r.status_code != 200:
        return 1

    mod = load_form_parser()
    form = mod.FormParser()
    form.feed(r.text)
    if not form.found:
        print("没有 takeedit 表单（可能无编辑权限）")
        return 1
    data = dict(form.data)
    changed = []
    tag_field = find_tag_field(r.text)
    tags = list(form.tags)
    for tag in args.add_tag:
        if tag not in tags:
            tags.append(tag)
    for tag in args.remove_tag:
        if tag in tags:
            tags.remove(tag)
    # 始终回填已勾选的标签：否则只改简介/技术信息时，标签字段会从 POST 里消失。
    if tag_field and (form.tags or args.add_tag or args.remove_tag):
        data[tag_field] = tags
        if args.add_tag or args.remove_tag:
            changed.append(tag_field)
    if (args.add_tag or args.remove_tag) and not tag_field:
        print("编辑表单里没有标签复选框字段，无法改标签")
        return 1

    for label, prefix, value in (
        ("音频编码", "audiocodec_sel", args.audio_codec),
        ("片源类型", "source_sel", args.source),
        ("处理格式", "processing_sel", args.processing),
        ("制作组", "team_sel", args.team),
        ("视频编码", "codec_sel", args.codec),
        ("分辨率", "standard_sel", args.standard),
    ):
        if value is not None:
            key = one_field(data, prefix)
            if not key:
                print(f"编辑表单里没有 {label} 字段 {prefix}")
                return 1
            if not option_exists(r.text, key, value):
                print(f"{label} 字段 {key} 没有 value={value} 的选项，拒绝写入")
                return 1
            data[key] = value
            changed.append(f"{label}={value}")
    if args.name is not None:
        data["name"] = args.name
        changed.append("name")
    if args.subtitle is not None:
        data["small_descr"] = args.subtitle
        changed.append("small_descr")
    for pair in args.extra_field:
        if "=" not in pair:
            print(f"--extra-field 格式应为 NAME=VALUE：{pair}")
            return 1
        key, value = pair.split("=", 1)
        data[key] = value
        changed.append(key)
    if args.descr_file:
        new_descr = Path(args.descr_file).read_text(encoding="utf-8")
        field = next((f for f in DESCR_FIELDS if f in data), None)
        if not field:
            print("没有简介字段")
            return 1
        data[field] = new_descr
        changed.append(field)
    if args.technical_info_file:
        field = one_field(data, "technical_info") or next(
            (f for f in data if "technical_info" in f), None
        )
        if not field:
            print("没有 technical_info 字段")
            return 1
        tech = Path(args.technical_info_file).read_text(encoding="utf-8")
        if not tech.strip():
            print("技术信息文件是空的：不能把空 MediaInfo/BDInfo 提交上去")
            return 1
        data[field] = tech
        changed.append(field)
    elif args.clear_technical_info:
        for key in list(data):
            if "technical_info" in key:
                data[key] = ""
                changed.append(f"清空 {key}")
    if args.append_image:
        field = next((f for f in DESCR_FIELDS if f in data), None)
        if not field:
            print("没有简介字段，无法追加图片")
            return 1
        cur = str(data.get(field, ""))
        added = []
        for url in (u.strip() for u in args.append_image):
            if not url or url in cur:
                continue
            sep = "" if not cur or cur.endswith("\n") else "\n"
            cur += f"{sep}[img]{url}[/img]\n"
            added.append(url)
        if added:
            data[field] = cur
            changed.append(f"{field} 追加图片 {len(added)} 张")
        else:
            print("追加图片全部已在简介里，未改动")

    print("将修改：" + ", ".join(changed))
    print(f"简介长度：{len(data.get('descr', ''))} 字节")
    tech_len = next(
        (len(v) for k, v in data.items() if "technical_info" in k), 0
    )
    print(f"技术信息长度：{tech_len} 字节")
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
