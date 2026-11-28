#!/usr/bin/env python3
"""按站点汇总已发种数量与最后一次发种时间。

只读 auto-audit.jsonl + reseed-log.jsonl：
- 只算成功上传类结果 posted / posted_verifying / pending_review / posted_unresolved
- URL 带 existed=1 的历史记录不算新发种
- 同一站点 + 同一内容键只算一条，避免两份日志重复计数
- 最后一次时间取该站去重后成功记录里最大的 at
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from datetime import datetime, timezone, timedelta
from pathlib import Path

SUCCESS = {"posted", "posted_verifying", "pending_review", "posted_unresolved"}
CST = timezone(timedelta(hours=8))


def is_new_post(row: dict) -> bool:
    if str(row.get("result") or "") not in SUCCESS:
        return False
    # 历史版本把站点回包 existed=1 也记成 posted；链接指向原有同 hash 条目，不算新发。
    return "existed=1" not in str(row.get("url") or "")


def read_jsonl(path: Path):
    if not path.exists():
        return []
    rows = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows


def unix_seconds(value) -> int:
    if isinstance(value, (int, float)):
        return int(value)
    if isinstance(value, str):
        return int(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp())
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--data-dir", default=str(Path.home() / "AppData/Local/BoxSend"))
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    data = Path(args.data_dir)

    seen = set()
    stats = defaultdict(lambda: {"posted": 0, "last_at": 0, "last_url": ""})
    for row in read_jsonl(data / "auto-audit.jsonl"):
        site = str(row.get("site") or "").strip().lower()
        key = str(row.get("key") or "").strip()
        if site and key and is_new_post(row):
            ident = (site, key)
            if ident not in seen:
                seen.add(ident)
                stats[site]["posted"] += 1
            at = unix_seconds(row.get("at"))
            if at >= stats[site]["last_at"]:
                stats[site]["last_at"] = at
                stats[site]["last_url"] = str(row.get("url") or "")
    for row in read_jsonl(data / "reseed-log.jsonl"):
        site = str(row.get("target_site") or "").strip().lower()
        key = str(row.get("key") or "").strip()
        if site and key and is_new_post(row):
            ident = (site, key)
            if ident in seen:
                # 同一内容在两份日志都有：数量不重复加，但最后时间要取更大的
                pass
            else:
                seen.add(ident)
                stats[site]["posted"] += 1
        if site and key and is_new_post(row):
            at = unix_seconds(row.get("at"))
            if at >= stats[site]["last_at"]:
                stats[site]["last_at"] = at
                stats[site]["last_url"] = str(row.get("url") or "")

    rows = []
    for site, s in stats.items():
        rows.append({
            "site": site,
            "posted": s["posted"],
            "last_at": s["last_at"],
            "last_at_text": datetime.fromtimestamp(s["last_at"], CST).strftime("%Y-%m-%d %H:%M") if s["last_at"] else "",
            "last_url": s["last_url"],
        })
    rows.sort(key=lambda r: (-r["posted"], -r["last_at"], r["site"]))
    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=2))
        return 0
    total = sum(r["posted"] for r in rows)
    print(f"站点 {len(rows)} 个，去重后成功发种 {total} 条\n")
    print("| 站点 | 已发种 | 最后发种时间 | 最后一条 |")
    print("|------|--------|--------------|----------|")
    for r in rows:
        print(f"| {r['site']} | {r['posted']} | {r['last_at_text']} | {r['last_url']} |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
