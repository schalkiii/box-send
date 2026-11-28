#!/usr/bin/env python3
"""清理已发布种子的简介残留：源站免责声明、图片外 [url] 包裹、简介里的技术参数块。

用法：
  py -3 scripts/clean-live-descr.py --site cspt --id 259870
  py -3 scripts/clean-live-descr.py --site cspt --id 259870 --apply

默认只读演练：抓 edit.php、打印将删除/改写的片段和输出文件；`--apply` 时调用
edit-seed.py 提交简介，其它字段原样回填。技术参数栏（technical_info）不动。
"""
from __future__ import annotations

import argparse
import html
import json
import os
import re
import sys
from pathlib import Path

import requests

BASE = Path(os.environ["LOCALAPPDATA"]) / "BoxSend"
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))


def site_url(site: str) -> str:
    cfg = json.loads((BASE / "boxsend.json").read_text(encoding="utf-8"))
    for s in cfg.get("sourceSites", []):
        if s.get("id") == site:
            return s["url"].rstrip("/")
    raise SystemExit(f"配置里找不到站点 {site}")


def cookies_for(host: str) -> dict[str, str]:
    data = json.loads((BASE / "cookies.json").read_text(encoding="utf-8"))
    h = host.lower()
    out = {}
    for key, arr in data.items():
        k = key.lower().lstrip(".")
        if h == k or h.endswith("." + k):
            for c in arr:
                if c.get("name") and c.get("value"):
                    out[c["name"]] = c["value"]
    return out


def fetch_descr(site: str, tid: str) -> tuple[str, str]:
    base = site_url(site)
    host = re.sub(r"^https?://", "", base).split("/")[0]
    url = f"{base}/edit.php?id={tid}"
    r = requests.get(
        url,
        cookies=cookies_for(host),
        headers={"User-Agent": "Mozilla/5.0"},
        timeout=(10, 30),
    )
    r.raise_for_status()
    m = re.search(r'<textarea[^>]*name="descr"[^>]*>(.*?)</textarea>', r.text, re.S | re.I)
    if not m:
        raise SystemExit("编辑页没有 descr 文本域，可能没登录或不是可编辑种子")
    return url, html.unescape(m.group(1))


def strip_source_quotes(text: str) -> tuple[str, int]:
    """删掉引用块里带源站 bare host（如 1PTBA）的整段。目标站自己的 quote 保留。"""
    n = 0

    def repl(m: re.Match[str]) -> str:
        nonlocal n
        block = m.group(0)
        inner = m.group(1)
        if re.search(r"(?i)(?<![a-z0-9.-])(?:1ptba|hdsky|hds\.me|imgbox|pthome|ourbits|hdbits|chdbits)\b", inner):
            n += 1
            return "\n"
        return block

    out = re.sub(r"\[quote\]([\s\S]*?)\[/quote\]", repl, text, flags=re.I)
    return out, n


def unwrap_image_urls(text: str) -> tuple[str, int]:
    """[url=图片][img]图片[/img][/url] -> [img]图片[/img]。只处理 URL 与图片地址相同的包裹。"""
    n = 0

    def repl(m: re.Match[str]) -> str:
        nonlocal n
        link, img = m.group(1), m.group(2)
        if link.strip() == img.strip():
            n += 1
            return f"[img]{img}[/img]"
        return m.group(0)

    out = re.sub(
        r"\[url=([^\]\s]+)\]\s*\[img\]([^\[\]]+)\[/img\]\s*\[/url\]",
        repl,
        text,
        flags=re.I,
    )
    return out, n


def strip_tech_block(text: str) -> tuple[str, int]:
    """删掉简介里的“参数信息/MediaInfo”块，保留后面的截图段。"""
    n = 0
    patterns = [
        r"※{3,}\s*参数信息\s*※{3,}[\s\S]*?(?=※{3,}\s*截图信息\s*※{3,}|$)",
        r"\[mediainfo\][\s\S]*?\[/mediainfo\]",
        r"\[hide\][\s\S]*?MediaInfo[\s\S]*?\[/hide\]",
    ]
    for p in patterns:
        text, k = re.subn(p, "\n", text, flags=re.I)
        n += k
    return text, n


def check_images(text: str) -> list[tuple[str, str]]:
    out = []
    for m in re.finditer(r"\[img\]([^\[\]]+)\[/img\]", text, re.I):
        u = m.group(1).strip()
        try:
            r = requests.get(
                u,
                headers={"User-Agent": "Mozilla/5.0", "Referer": ""},
                timeout=(8, 15),
                stream=True,
            )
            code = str(r.status_code)
            r.close()
        except Exception as e:  # noqa: BLE001
            code = type(e).__name__
        out.append((u, code))
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--site", required=True)
    ap.add_argument("--id", required=True)
    ap.add_argument("--apply", action="store_true")
    ap.add_argument("--keep-tech-block", action="store_true")
    args = ap.parse_args()

    url, descr = fetch_descr(args.site, args.id)
    original = descr
    descr, q = strip_source_quotes(descr)
    descr, u = unwrap_image_urls(descr)
    t = 0
    if not args.keep_tech_block:
        descr, t = strip_tech_block(descr)
    descr = re.sub(r"\n{3,}", "\n\n", descr).strip() + "\n"

    out_dir = HERE.parent / "artifacts"
    out_dir.mkdir(exist_ok=True)
    out = out_dir / f"clean-descr-{args.site}-{args.id}.txt"
    out.write_text(descr, encoding="utf-8")

    print(f"编辑页 {url}")
    print(f"原简介 {len(original)} 字节；清理后 {len(descr)} 字节")
    print(f"删源站引用块 {q} 段，拆图片 URL 包裹 {u} 处，删技术参数块 {t} 处")
    print(f"输出 {out}")
    for u, code in check_images(descr):
        print(f"图片 {code} {u}")
    if args.apply:
        import subprocess

        cmd = [
            sys.executable,
            str(HERE / "edit-seed.py"),
            "--site",
            args.site,
            "--id",
            args.id,
            "--descr-file",
            str(out),
            "--apply",
        ]
        print("提交：", " ".join(cmd))
        raise SystemExit(subprocess.call(cmd))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
