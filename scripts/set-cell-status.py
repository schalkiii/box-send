#!/usr/bin/env python3
r"""改覆盖记录里某一格的状态、URL 和说明（先备份，失败不写回）。

用法：
  py -3 scripts\set-cell-status.py <内容键> <站点> <状态> "<说明>" ["<详情页 URL>"]

状态必须是 index.rs 里的值，例如 other_version / assisted / skipped /
review_pending / pending_review。只动指定站点格，不改其他内容。
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import time
from pathlib import Path

P = Path(os.environ["LOCALAPPDATA"]) / "BoxSend" / "coverage.json"
VALID = {
    "seeding",
    "assisted",
    "reseeded",
    "verifying",
    "review_pending",
    "other_version",
    "same_hash_skip",
    "rejected",
    "miss",
    "wait_source",
    "failed",
    "reseed_draft",
    "skipped",
    "pending_offers",
    "pending_review",
    "review_rejected",
    "gone_from_site",
}


def main() -> int:
    if len(sys.argv) not in (5, 6):
        print(__doc__)
        return 2
    key, site, status, note = sys.argv[1:5]
    url = sys.argv[5] if len(sys.argv) == 6 else ""
    if status not in VALID:
        print(f"未知状态 {status}")
        return 2

    data = json.loads(P.read_text(encoding="utf-8"))
    entry = data.get("contents", {}).get(key)
    if not entry:
        print(f"找不到内容键 {key}")
        return 1
    cell = entry.get("coverage", {}).get(site)
    if not cell:
        print(f"{key} 下找不到站点格 {site}")
        return 1

    bak = P.with_name(f"coverage.json.bak-{time.strftime('%Y%m%d-%H%M%S')}")
    shutil.copy2(P, bak)

    cell["status"] = status
    cell["at"] = int(time.time())
    cell["note"] = note
    if url:
        cell["url"] = url
    P.write_text(
        json.dumps(data, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="",
    )
    print(f"写回完成：{key} / {site} -> {status}；备份 {bak.name}")
    print(json.dumps(cell, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
