#!/usr/bin/env python3
r"""按区间改覆盖记录里某一格的 note（不整份重写，保持原换行与重复键原样）。

用法： py -3 scripts\set-cell-note.py m:5826983b00126e58 qilin "新说明"
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import time
from pathlib import Path

P = Path(os.environ["LOCALAPPDATA"]) / "BoxSend" / "coverage.json"


def main() -> int:
    if len(sys.argv) != 4:
        print("用法：set-cell-note.py <内容键> <站点> <新 note>")
        return 2
    key, site, note = sys.argv[1], sys.argv[2], sys.argv[3]
    raw = P.read_text(encoding="utf-8", newline="")

    i = raw.find(f'"{key}"')
    if i < 0:
        print(f"找不到内容键 {key}")
        return 1
    j = raw.find(f'"{site}"', i)
    if j < 0 or j > i + 200000:
        print(f"{key} 下找不到站点格 {site}")
        return 1
    k = raw.find('"note":', j)
    if k < 0:
        print("找不到 note 字段")
        return 1
    q0 = raw.find('"', k + 7)
    if q0 < 0:
        return 1
    q1 = q0 + 1
    while q1 < len(raw):
        if raw[q1] == "\\":
            q1 += 2
            continue
        if raw[q1] == '"':
            break
        q1 += 1
    old = raw[q0:q1 + 1]

    bak = P.with_name(f"coverage.json.bak-{time.strftime('%Y%m%d-%H%M%S')}")
    shutil.copy2(P, bak)

    new = json.dumps(note, ensure_ascii=False)
    raw2 = raw[:q0] + new + raw[q1 + 1:]
    P.write_text(raw2, encoding="utf-8", newline="")
    print("旧 note:", old)
    print("新 note:", new)
    print(f"写回完成，{len(raw)} -> {len(raw2)} 字节；备份 {bak.name}")

    data = json.loads(P.read_text(encoding="utf-8", errors="replace"))
    cell = data["contents"][key]["coverage"][site]
    print("复核:", json.dumps(cell, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
