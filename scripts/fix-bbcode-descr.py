#!/usr/bin/env python3
"""把已发布种子的 HTML 简介改成 BBCode（仅改简介，不换种子文件）。

用法：
  py -3 scripts/fix-bbcode-descr.py              # 只读演练：抓编辑页、打印要改的条数
  py -3 scripts/fix-bbcode-descr.py --apply      # 真提交；逐条核对详情页
  py -3 scripts/fix-bbcode-descr.py --apply --site sbpt

数据来源：
  %LOCALAPPDATA%\\BoxSend\\coverage.json   已发布种子的详情页
  %LOCALAPPDATA%\\BoxSend\\cookies.json    各站登录 Cookie

只处理 NexusPHP 的 takeedit.php 表单；找不到编辑表单、简介已经是 BBCode、
或简介不是 HTML 的条目会跳过。
"""
from __future__ import annotations

import argparse
import html
import json
import os
import re
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urljoin, urlparse

import requests

BASE = Path(os.environ["LOCALAPPDATA"]) / "BoxSend"
DEBUG = BASE / "upload-debug" / "descr-fix"
DESCR_FIELDS = ("descr", "release_desc", "description", "content")


class FormParser(HTMLParser):
    """只抓 takeedit 表单里的控件；保留浏览器默认提交值。"""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.in_form = False
        self.found = False
        self.depth = 0
        self.data: dict[str, str] = {}
        self.tags: list[str] = []
        self.cur_select = None
        self.cur_textarea = None
        self.text: list[str] = []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        tag = tag.lower()
        if tag == "form":
            if "takeedit" in (a.get("action") or ""):
                self.in_form = True
                self.found = True
                self.depth = 1
            elif self.in_form:
                self.depth += 1
            return
        if not self.in_form:
            return
        if tag == "input":
            name = a.get("name")
            if not name:
                return
            typ = (a.get("type") or "text").lower()
            if typ in ("submit", "button", "image", "reset", "file"):
                return
            if typ in ("checkbox", "radio"):
                if "checked" in a:
                    if name.startswith("tags["):
                        self.tags.append(a.get("value", ""))
                    else:
                        self.data[name] = a.get("value", "on")
                return
            self.data[name] = a.get("value", "")
        elif tag == "textarea":
            self.cur_textarea = a.get("name")
            self.text = []
        elif tag == "select":
            self.cur_select = {
                "name": a.get("name"),
                "selected": None,
                "first": None,
                "pending": None,
            }
        elif tag == "option" and self.cur_select is not None:
            p = {"value": a.get("value"), "text": ""}
            self.cur_select["pending"] = p
            if self.cur_select["first"] is None:
                self.cur_select["first"] = p
            if "selected" in a:
                self.cur_select["selected"] = p

    def handle_data(self, data):
        if not self.in_form:
            return
        if self.cur_textarea is not None:
            self.text.append(data)
        if self.cur_select is not None and self.cur_select["pending"] is not None:
            self.cur_select["pending"]["text"] += data

    def handle_endtag(self, tag):
        tag = tag.lower()
        if not self.in_form:
            return
        if tag == "textarea" and self.cur_textarea is not None:
            self.data[self.cur_textarea] = "".join(self.text)
            self.cur_textarea = None
            self.text = []
        elif tag == "select" and self.cur_select is not None:
            sel = self.cur_select["selected"] or self.cur_select["first"]
            if self.cur_select["name"] and sel:
                value = sel["value"] if sel["value"] is not None else sel["text"].strip()
                self.data[self.cur_select["name"]] = value
            self.cur_select = None
        elif tag == "form":
            self.depth -= 1
            if self.depth <= 0:
                self.in_form = False


def clean_url(raw: str) -> str:
    u = (raw or "").strip()
    while True:
        if u.endswith("%2522"):
            u = u[:-6]
        elif u.endswith("%22"):
            u = u[:-3]
        else:
            break
    while u.startswith("%22"):
        u = u[3:]
    return u.strip('"').strip().replace("]", "%5D")


def css_color(style: str) -> str | None:
    lower = (style or "").lower()
    p = lower.find("color:")
    if p < 0:
        return None
    rest = (style or "")[p + len("color:") :]
    value = re.split(r"[;!]", rest)[0].strip().strip('"')
    return value or None


class Bbcode(HTMLParser):
    """够用就好的 HTML→BBCode，和 Rust 端保持同一组标签。"""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.out: list[str] = []
        self.stack: list[tuple[str, str]] = []
        self.skip_legend = 0

    def nl(self) -> None:
        if self.out and not self.out[-1].endswith("\n"):
            self.out.append("\n")

    def open(self, key: str, text: str, close: str) -> None:
        self.out.append(text)
        self.stack.append((key, close))

    def close(self, key: str) -> None:
        for i in range(len(self.stack) - 1, -1, -1):
            if self.stack[i][0] == key:
                while len(self.stack) > i:
                    self.out.append(self.stack.pop()[1])
                return

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        tag = tag.lower()
        if tag == "legend":
            self.skip_legend += 1
            return
        if tag == "br":
            self.nl()
            return
        if tag in ("div", "p", "center", "table", "tbody", "thead", "ul", "ol", "tr", "pre"):
            self.nl()
            return
        if tag in ("td", "th"):
            if self.out and not self.out[-1].endswith((" ", "\n")):
                self.out.append(" ")
            return
        if tag == "li":
            self.nl()
            self.out.append("• ")
            return
        if tag in ("fieldset", "blockquote"):
            self.nl()
            self.open("quote", "[quote]\n", "\n[/quote]")
            return
        if tag in ("b", "strong"):
            self.open("b", "[b]", "[/b]")
            return
        if tag in ("i", "em"):
            self.open("i", "[i]", "[/i]")
            return
        if tag == "u":
            self.open("u", "[u]", "[/u]")
            return
        if tag == "code":
            self.open("code", "[code]", "[/code]")
            return
        if tag in ("h1", "h2", "h3", "h4", "h5", "h6"):
            self.nl()
            self.open("h", "[b]", "[/b]")
            return
        if tag == "a":
            href = (a.get("href") or "").strip()
            if href and href != "#":
                self.open("a", "[url=%s]" % href.replace("]", "%5D"), "[/url]")
            return
        if tag == "img":
            u = clean_url(a.get("src"))
            if u.startswith(("http://", "https://")):
                self.out.append("[img]%s[/img]" % u)
            return
        if tag == "font":
            color = (a.get("color") or "").strip()
            size = a.get("size") or ""
            opens = ""
            closes = ""
            if color:
                opens += "[color=%s]" % color
                closes += "[/color]"
            if size.isdigit() and 1 <= int(size) <= 7:
                opens += "[size=%s]" % size
                closes = "[/size]" + closes
            if opens:
                self.open("font", opens, closes)
            return
        if tag == "span":
            color = css_color(a.get("style"))
            if color:
                self.open("span", "[color=%s]" % color, "[/color]")

    def handle_endtag(self, tag):
        tag = tag.lower()
        if tag == "legend":
            self.skip_legend = max(0, self.skip_legend - 1)
            return
        if tag in ("div", "p", "center", "table", "tbody", "thead", "ul", "ol", "tr", "pre"):
            self.nl()
            return
        if tag in ("fieldset", "blockquote"):
            self.nl()
            self.close("quote")
            return
        if tag in ("b", "strong"):
            self.close("b")
            return
        if tag in ("i", "em"):
            self.close("i")
            return
        if tag == "u":
            self.close("u")
            return
        if tag == "code":
            self.close("code")
            return
        if tag in ("h1", "h2", "h3", "h4", "h5", "h6"):
            self.nl()
            self.close("h")
            return
        if tag == "a":
            self.close("a")
            return
        if tag == "font":
            self.close("font")
            return
        if tag == "span":
            self.close("span")

    def handle_data(self, data):
        if not self.skip_legend:
            self.out.append(data)


def html_to_bbcode(raw: str) -> str:
    parser = Bbcode()
    parser.feed(raw)
    text = "".join(parser.out).replace("\r\n", "\n").replace("\r", "\n")
    lines: list[str] = []
    blank = 0
    for line in text.split("\n"):
        line = line.rstrip()
        if not line.strip():
            blank += 1
            if blank <= 1:
                lines.append("")
        else:
            blank = 0
            lines.append(line)
    return "\n".join(lines).strip()


def looks_like_html(raw: str) -> bool:
    lower = raw.lower()
    return any(
        t in lower
        for t in (
            "<fieldset",
            "<img",
            "<br",
            "<b>",
            "<strong",
            "<font",
            "<span",
            "<div",
            "<p>",
            "<a ",
            "<center",
            "<table",
            "<blockquote",
            "<pre",
            "<code",
            "<i>",
            "<u>",
        )
    )


def find_descr(data: dict[str, str]) -> str | None:
    for name in DESCR_FIELDS:
        if name in data:
            return name
    return None


def session_for(host: str, cookies: dict) -> requests.Session:
    session = requests.Session()
    for c in cookies.get(host, []):
        session.cookies.set(c["name"], c["value"], domain=host)
    return session


def fix_one(site: str, url: str, apply: bool, cookies: dict) -> dict:
    host = urlparse(url).hostname or ""
    edit_url = urljoin(url, "edit.php?id=" + (url.rsplit("=", 1)[-1]))
    row = {"site": site, "url": url, "edit": edit_url, "status": "skip", "note": ""}
    s = session_for(host, cookies)
    r = s.get(edit_url, headers={"User-Agent": "Mozilla/5.0"}, timeout=30)
    row["http"] = r.status_code
    if r.status_code != 200:
        row["note"] = "编辑页 HTTP %s" % r.status_code
        return row
    form = FormParser()
    form.feed(r.text)
    if not form.found:
        row["note"] = "没有 takeedit 表单（可能无编辑权限）"
        return row
    data = form.data
    if form.tags:
        data["tags[4][]"] = form.tags
    field = find_descr(data)
    if not field:
        row["note"] = "没有简介字段"
        return row
    raw = data.get(field, "")
    row["raw_len"] = len(raw)
    if not looks_like_html(raw):
        row["note"] = "简介已经是纯文本/BBCode"
        return row
    bb = html_to_bbcode(raw)
    row["bb_len"] = len(bb)
    row["field"] = field
    if not apply:
        row["status"] = "would-fix"
        return row
    DEBUG.mkdir(parents=True, exist_ok=True)
    stem = re.sub(r"[^0-9A-Za-z_.-]+", "_", site + "-" + url.rsplit("=", 1)[-1])
    (DEBUG / (stem + "-before.html")).write_text(r.text, encoding="utf-8")
    data[field] = bb
    resp = s.post(
        urljoin(edit_url, "takeedit.php"),
        data=data,
        headers={"User-Agent": "Mozilla/5.0", "Referer": edit_url},
        allow_redirects=False,
        timeout=30,
    )
    row["post"] = resp.status_code
    if resp.status_code in (301, 302, 303):
        detail_url = urljoin(edit_url, resp.headers.get("Location", url))
        detail = s.get(detail_url, headers={"User-Agent": "Mozilla/5.0"}, timeout=30)
        row["detail_http"] = detail.status_code
        text = detail.text
        m = re.search(r"id=['\"]kdescr['\"]", text)
        seg = text[m.start() : m.start() + 16000] if m else text
        row["escaped"] = seg.count("&lt;fieldset") + seg.count("&lt;img")
        row["rendered"] = seg.count("<fieldset") + seg.count("<img")
        row["status"] = "fixed" if row["escaped"] == 0 else "check"
        if row["escaped"]:
            row["note"] = "详情页仍见转义标签"
    else:
        # 部分站 200 + 空回包也算成功；用详情页复核
        detail = s.get(url, headers={"User-Agent": "Mozilla/5.0"}, timeout=30).text
        m = re.search(r"id=['\"]kdescr['\"]", detail)
        seg = detail[m.start() : m.start() + 16000] if m else detail
        row["escaped"] = seg.count("&lt;fieldset") + seg.count("&lt;img")
        row["rendered"] = seg.count("<fieldset") + seg.count("<img")
        row["status"] = "fixed" if row["escaped"] == 0 and row["rendered"] > 0 else "check"
        if row["escaped"]:
            row["note"] = "提交后详情页仍见转义标签"
    return row


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--apply", action="store_true", help="真提交（默认只读演练）")
    ap.add_argument("--site", action="append", help="只处理指定站点，可重复")
    args = ap.parse_args()
    coverage = json.loads((BASE / "coverage.json").read_text(encoding="utf-8"))
    cookies = json.loads((BASE / "cookies.json").read_text(encoding="utf-8"))
    rows = []
    for _, content in coverage["contents"].items():
        for site, rec in (content.get("coverage") or {}).items():
            if rec.get("status") != "reseeded" or not rec.get("url"):
                continue
            if args.site and site not in args.site:
                continue
            rows.append((site, rec["url"]))
    print("待检查 %d 条；%s" % (len(rows), "真提交" if args.apply else "只读演练"))
    results = []
    for site, url in sorted(rows):
        try:
            row = fix_one(site, url, args.apply, cookies)
        except Exception as exc:  # 单站失败不拖垮整批
            row = {"site": site, "url": url, "status": "error", "note": str(exc)}
        results.append(row)
        print("%-10s %-10s %s" % (site, row["status"], row.get("note", "")), flush=True)
    print("\n汇总：")
    for row in results:
        print(json.dumps(row, ensure_ascii=False))
    fixed = sum(r["status"] == "fixed" for r in results)
    print("\nfixed=%d check=%d skip=%d error=%d" % (
        fixed,
        sum(r["status"] == "check" for r in results),
        sum(r["status"] == "skip" for r in results),
        sum(r["status"] == "error" for r in results),
    ))
    return 0 if all(r["status"] in ("fixed", "skip", "would-fix") for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
