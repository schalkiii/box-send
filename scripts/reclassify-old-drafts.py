#!/usr/bin/env python3
"""把历史“批量测试清理/旧草稿”失败项从 failed 改成 skipped。

这些不是站点拒绝，只是早先批量演练留下的草稿；继续挂在 failed 里会掩盖真实失败。
只动 note 命中旧草稿字样的格子，一次备份，不改 URL。
"""
from __future__ import annotations

import json
import os
import shutil
import time
from pathlib import Path

BASE = Path(os.environ["LOCALAPPDATA"]) / "BoxSend"
PATH = BASE / "coverage.json"
MARKERS = ("旧草稿", "清理", "批量测试", "程序化流程测试")


def main() -> int:
    data = json.loads(PATH.read_text(encoding="utf-8"))
    changed = 0
    for key, entry in data.get("contents", {}).items():
        for site, cell in entry.get("coverage", {}).items():
            if cell.get("status") != "failed":
                continue
            note = str(cell.get("note", ""))
            if not any(m in note for m in MARKERS):
                continue
            cell["status"] = "skipped"
            cell["note"] = "早先批量演练留下的旧草稿，不是站点拒绝；要发需重新生成草稿"
            changed += 1
            print(f"{key} / {site}")
    if not changed:
        print("没有需要改的格子")
        return 0
    bak = PATH.with_name(f"coverage.json.bak-{time.strftime('%Y%m%d-%H%M%S')}")
    shutil.copyfile(PATH, bak)
    tmp = PATH.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    tmp.replace(PATH)
    print(f"改 {changed} 格；备份 {bak.name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
