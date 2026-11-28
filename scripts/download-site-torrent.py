#!/usr/bin/env python3
r"""用 cookies.json 的登录态下载站点 .torrent，供 `bsend pull --file` 挂回。

用法：
  py -3 scripts\download-site-torrent.py <站点 host> <详情页 URL> <输出文件>

只用于站点下载页能打开、但 bsend 的 HTTP 客户端被 Cloudflare 分块响应卡住时；
下载后仍由 bsend 在 qB 里按 pieces/文件清单核对，不直接手加 qB。
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

import requests

UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) BoxSend/1.0"


def cookies_for(host: str) -> dict[str, str]:
    path = Path(os.environ["LOCALAPPDATA"]) / "BoxSend" / "cookies.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    return {
        c["name"]: c["value"]
        for c in data.get(host, [])
        if isinstance(c, dict) and c.get("name")
    }


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__)
        return 2
    host, detail_url, out = sys.argv[1:4]
    cookies = cookies_for(host)
    s = requests.Session()
    s.headers["User-Agent"] = UA
    page = s.get(detail_url, cookies=cookies, timeout=(10, 30))
    page.raise_for_status()
    m = re.search(r'(?:href|src)=["\']([^"\']*download\.php\?id=[^"\']*)', page.text)
    if not m:
        print("详情页没找到 download.php?id= 链接")
        return 1
    url = requests.compat.urljoin(detail_url, m.group(1).replace("&amp;", "&"))
    r = s.get(url, cookies=cookies, timeout=(10, 30))
    r.raise_for_status()
    if not r.content.startswith(b"d8:announce"):
        print("返回内容不像 .torrent：", r.content[:80])
        return 1
    Path(out).write_bytes(r.content)
    print(f"已下载 {len(r.content)} 字节 -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
