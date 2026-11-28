#!/usr/bin/env python3
r"""给“源站简介没有图”的种子补图：本机视频抽帧（可选豆瓣海报），上传图床，
写入 screenshot-overrides.json。程序发布时会自动把这些图追加到简介末尾。

用法：
  # 指定内容键和本地视频（可多条；每条取一帧，或单条按比例取多帧）
  py -3 scripts/seed-images.py --content m:5ddeceab290355c3 ^
      --video "K:\...\S01.E01...mkv" --video "K:\...\S07.E01...mkv" ^
      --video "K:\...\S10.E01...mkv" --count 3 --uploader pixhost

  # 不知内容键时，可先用 --find-url 在 coverage.json 里找
  py -3 scripts/seed-images.py --find-url "https://carpt.net/details.php?id=532"

  # 有豆瓣条目时，加 --douban；脚本自动取条目海报后再从视频抽帧
  py -3 scripts/seed-images.py --content m:... --video ... --douban 2030879

  # 已有海报原图时，也可直接给 --poster-url
  py -3 scripts/seed-images.py --content m:... --video ... --poster-url "https://img9.doubanio.com/..."

  # 同一部剧有多个内容键（HDTV / WEB-DL 分开算）时，把同一批图写到多个键；
  # 也可以按内容名子串自动找齐。
  py -3 scripts/seed-images.py --content m:... --video ... --also-content m:...
  py -3 scripts/seed-images.py --content m:... --video ... --for-name "白色巨塔"

  # 只看会抽哪几帧、不上传不写文件
  py -3 scripts/seed-images.py --content m:... --video ... --dry-run

说明：
  - 默认上传 pixhost.to（UBITS 明确接受；tey 也可用）。imgbox 作为可选备选。
  - 抽帧用本机 ffmpeg；脚本按常见 MoviePilot 路径找，找不到时用 FFMPEG 环境变量。
  - 写入前会保留 screenshot-overrides.json 里其他内容键。
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import requests

BASE = Path(os.environ["LOCALAPPDATA"]) / "BoxSend"
REPO = Path(__file__).resolve().parent
OUT = Path(tempfile.gettempdir()) / "bsend-seed-images"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) Gecko/20100101 Firefox/132.0"
FFMPEG_CANDIDATES = [
    Path("C:/CommonTools/MoviePilot-v3/runtime/ffmpeg/ffmpeg.exe"),
    Path("C:/CommonTools/MoviePilot-v3/runtime/ffmpeg/ffprobe.exe"),
]


def find_ffmpeg() -> tuple[Path, Path]:
    env = os.environ.get("FFMPEG")
    if env:
        p = Path(env)
        probe = p.with_name("ffprobe.exe" if p.suffix.lower() == ".exe" else "ffprobe")
        if p.is_file() and probe.is_file():
            return p, probe
    ffmpeg = shutil.which("ffmpeg")
    ffprobe = shutil.which("ffprobe")
    if ffmpeg and ffprobe:
        return Path(ffmpeg), Path(ffprobe)
    for p in FFMPEG_CANDIDATES:
        sibling = p.with_name("ffprobe.exe" if p.name == "ffmpeg.exe" else "ffmpeg.exe")
        candidate = p if p.name == "ffmpeg.exe" else sibling
        probe = candidate.with_name("ffprobe.exe")
        if candidate.is_file() and probe.is_file():
            return candidate, probe
    raise SystemExit("找不到 ffmpeg/ffprobe；设置 FFMPEG 环境变量指向 ffmpeg.exe")


def duration(ffprobe: Path, video: Path) -> float:
    p = subprocess.run(
        [str(ffprobe), "-v", "error", "-show_entries", "format=duration",
         "-of", "default=noprint_wrappers=1:nokey=1", str(video)],
        capture_output=True, text=True, timeout=60,
    )
    try:
        return float(p.stdout.strip())
    except ValueError:
        return 0.0


def extract_frame(ffmpeg: Path, video: Path, at: float, out: Path) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    p = subprocess.run(
        [str(ffmpeg), "-hide_banner", "-loglevel", "error", "-ss", f"{at:.3f}",
         "-i", str(video), "-frames:v", "1", "-vf", "scale=1280:-2",
         "-q:v", "2", "-y", str(out)],
        capture_output=True, text=True, timeout=240,
    )
    if p.returncode != 0 or not out.is_file() or out.stat().st_size == 0:
        raise RuntimeError(f"抽帧失败：{video} @ {at:.1f}s\n{p.stderr[-800:]}")


def upload_pixhost(path: Path) -> str:
    last: Exception | None = None
    for attempt in range(6):
        try:
            with path.open("rb") as fh:
                r = requests.post(
                    "https://api.pixhost.to/images",
                    data={"content_type": "0", "max_th_size": "420"},
                    files={"img": (path.name, fh, "image/jpeg")},
                    headers={"User-Agent": UA, "Accept": "application/json"},
                    timeout=20,
                )
            r.raise_for_status()
            data = r.json()
            show = data.get("show_url") or ""
            if not show:
                raise RuntimeError(f"pixhost 返回没有 show_url：{data}")
            # show 页 https://pixhost.to/show/7443/815...jpg
            # 原图直链 https://img3.pixhost.to/images/7443/815...jpg
            return re.sub(r"https://pixhost\.to/show/", "https://img3.pixhost.to/images/", show)
        except Exception as e:  # 实测 api.pixhost.to 偶发读超时，重试可恢复
            last = e
            if attempt < 5:
                time.sleep(4)
    raise last or RuntimeError("pixhost 上传失败")


def upload_imgbox(path: Path) -> str:
    spec = importlib.util.spec_from_file_location("imgbox_upload", REPO / "imgbox-upload.py")
    mod = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(mod)
    s, csrf = mod.create_session()
    gallery = mod.create_gallery(s, csrf)
    data = mod.upload(s, csrf, gallery, [path])
    files = data.get("files") or []
    if not files:
        raise RuntimeError(f"imgbox 没有返回文件：{data}")
    return files[0].get("original_url") or files[0].get("url")


def upload(path: Path, uploader: str) -> str:
    return upload_pixhost(path) if uploader == "pixhost" else upload_imgbox(path)


DOUBAN_API = "https://m.douban.com/rexxar/api/v2/{kind}/{subject}"
DOUBAN_HEADERS = {
    "User-Agent": "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) "
                  "AppleWebKit/605.1.15 Mobile/15E148",
    "Referer": "https://m.douban.com/",
}


def douban_subject_id(value: str) -> str:
    m = re.search(r"/subject/(\d+)", value) or re.search(r"(\d{4,})", value)
    if not m:
        raise SystemExit(f"认不出豆瓣条目：{value}；请传条目 id 或 subject URL")
    return m.group(1)


def douban_poster(value: str) -> str:
    """从豆瓣移动接口取条目海报原图。电影和剧集各试一次，只取 pic.large。"""
    subject = douban_subject_id(value)
    last = ""
    for kind in ("movie", "tv"):
        url = DOUBAN_API.format(kind=kind, subject=subject)
        r = requests.get(url, headers=DOUBAN_HEADERS, timeout=30)
        if r.status_code != 200:
            last = f"{kind}: HTTP {r.status_code}"
            continue
        try:
            data = r.json()
        except ValueError:
            last = f"{kind}: 返回不是 JSON"
            continue
        pic = data.get("pic") or {}
        poster = pic.get("large") if isinstance(pic, dict) else ""
        poster = poster or data.get("cover_url") or ""
        if poster:
            return poster
        last = f"{kind}: 没有 pic.large/cover_url"
    raise SystemExit(f"豆瓣条目 {subject} 取不到海报（{last}）")


def resolve_content_key(source_url: str) -> str:
    # coverage 里只存站点键，不存详情 URL；用 bsend 的自动审计/转种日志反查
    for name in ("auto-audit.jsonl", "reseed-log.jsonl"):
        p = BASE / name
        if not p.exists():
            continue
        for line in p.read_text(encoding="utf-8").splitlines():
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if source_url in json.dumps(row, ensure_ascii=False):
                for k in ("content_key", "contentKey", "key"):
                    if row.get(k):
                        return str(row[k])
    raise SystemExit("在审计/转种日志里找不到这个详情 URL 对应的内容键；请直接传 --content")


def qb_videos(content_key: str) -> list[Path]:
    """从 qB 里找该内容键对应的视频文件绝对路径（取一个已完成的种子）。"""
    cfg = json.loads((BASE / "boxsend.json").read_text(encoding="utf-8"))["downloader"]
    base = cfg["url"].rstrip("/")
    s = requests.Session()
    s.headers.update({"User-Agent": UA})
    s.post(base + "/api/v2/auth/login", data={"username": cfg["username"], "password": cfg["password"]}, timeout=30).raise_for_status()
    idx = json.loads((BASE / "coverage.json").read_text(encoding="utf-8"))
    entry = idx["contents"].get(content_key)
    if not entry:
        raise SystemExit(f"coverage.json 里没有内容键 {content_key}")
    for h, meta in entry.get("torrents", {}).items():
        info = s.get(base + "/api/v2/torrents/info", params={"hashes": h}, timeout=30).json()
        if not info:
            continue
        files = s.get(base + "/api/v2/torrents/files", params={"hash": h}, timeout=60).json()
        vids = [f for f in files if f.get("name", "").lower().endswith((".mkv", ".mp4", ".ts"))]
        if not vids:
            continue
        v = max(vids, key=lambda x: x.get("size", 0))
        path = Path(info[0]["save_path"]) / Path(v["name"].replace("/", os.sep))
        if path.is_file():
            return [path]
    raise SystemExit("qB 里没找到这个内容键的完整视频；请用 --video 指定")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--content", help="coverage.json 的内容键，例如 m:5dde...")
    ap.add_argument("--also-content", action="append", default=[], help="同一批图也写入这些内容键（可重复）")
    ap.add_argument("--for-name", help="按 coverage.json 内容名子串匹配，把同一批图写入所有匹配内容键")
    ap.add_argument("--find-url", help="按详情 URL 在审计/转种日志里反查内容键")
    ap.add_argument("--video", action="append", help="本地视频路径，可重复；不传则从 qB 找")
    ap.add_argument("--count", type=int, default=3, help="单条视频抽几帧（默认 3）")
    ap.add_argument("--fractions", default="0.15,0.5,0.85", help="单条视频抽帧时间比例")
    ap.add_argument("--poster-url", help="海报原图 URL（豆瓣图会带 Referer 下载）")
    ap.add_argument("--poster-file", help="本机已有海报文件，直接上传，不再下载")
    ap.add_argument("--douban", help="豆瓣条目 id 或 subject URL；自动取条目海报")
    ap.add_argument("--uploader", choices=["pixhost", "imgbox"], default="pixhost")
    ap.add_argument("--out", default=str(BASE / "screenshot-overrides.json"))
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    content = args.content or (resolve_content_key(args.find_url) if args.find_url else "")
    if not content:
        raise SystemExit("必须给 --content 或 --find-url")
    targets = [content, *args.also_content]
    if args.for_name:
        idx = json.loads((BASE / "coverage.json").read_text(encoding="utf-8"))
        matched = [
            key
            for key, entry in (idx.get("contents") or {}).items()
            if args.for_name in str(entry.get("name", ""))
        ]
        if not matched:
            raise SystemExit(f"coverage.json 里没有名字含「{args.for_name}」的内容键")
        targets.extend(matched)
    targets = list(dict.fromkeys(targets))
    videos = [Path(v) for v in args.video] if args.video else qb_videos(content)
    missing = [str(v) for v in videos if not v.is_file()]
    if missing:
        raise SystemExit("视频不存在：" + ", ".join(missing))

    ffmpeg, ffprobe = find_ffmpeg()
    OUT.mkdir(parents=True, exist_ok=True)
    shots: list[Path] = []
    safe = content.replace(":", "_")

    poster_url = args.poster_url or (douban_poster(args.douban) if args.douban else "")
    if args.poster_file and poster_url:
        raise SystemExit("--poster-file 和 --poster-url/--douban 只能选一个")
    if args.poster_file:
        src = Path(args.poster_file)
        if not src.is_file() or src.stat().st_size == 0:
            raise SystemExit(f"海报文件不存在或为空：{src}")
        poster = OUT / f"{safe}-poster{src.suffix.lower() or '.jpg'}"
        shutil.copyfile(src, poster)
        shots.append(poster)
    elif poster_url:
        poster = OUT / f"{safe}-poster.jpg"
        r = requests.get(
            poster_url,
            headers={"User-Agent": UA, "Referer": "https://movie.douban.com/"},
            timeout=60,
        )
        r.raise_for_status()
        poster.write_bytes(r.content)
        if poster.stat().st_size == 0:
            raise SystemExit("海报下载为空")
        shots.append(poster)

    if len(videos) == 1:
        dur = duration(ffprobe, videos[0])
        if dur <= 0:
            raise SystemExit(f"读不到视频时长：{videos[0]}")
        fractions = [float(x.strip()) for x in args.fractions.split(",") if x.strip()]
        for i, frac in enumerate(fractions[: max(1, args.count)], 1):
            out = OUT / f"{safe}-{i}.jpg"
            extract_frame(ffmpeg, videos[0], dur * frac, out)
            shots.append(out)
    else:
        for i, video in enumerate(videos[: max(1, args.count)], 1):
            dur = duration(ffprobe, video)
            if dur <= 0:
                raise SystemExit(f"读不到视频时长：{video}")
            out = OUT / f"{safe}-{i}.jpg"
            extract_frame(ffmpeg, video, dur * 0.5, out)
            shots.append(out)

    print(f"内容键 {content}；抽帧 {len(shots)} 张：", file=sys.stderr)
    for p in shots:
        print(f"  {p} ({p.stat().st_size} 字节)", file=sys.stderr)
    if args.dry_run:
        print(
            json.dumps(
                {
                    "ok": True,
                    "dry_run": True,
                    "content": content,
                    "contents": targets,
                    "local": [str(p) for p in shots],
                },
                ensure_ascii=False,
            )
        )
        return 0

    urls = [upload(p, args.uploader) for p in shots]
    print("上传结果：", file=sys.stderr)
    for p, u in zip(shots, urls):
        print(f"  {p.name} -> {u}", file=sys.stderr)

    out = Path(args.out)
    old = {}
    if out.exists():
        try:
            old = json.loads(out.read_text(encoding="utf-8"))
        except json.JSONDecodeError as e:
            raise SystemExit(f"{out} 不是合法 JSON，拒绝覆盖：{e}")
    for key in targets:
        old[key] = urls
    out.write_text(json.dumps(old, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(
        json.dumps(
            {
                "ok": True,
                "content": content,
                "contents": targets,
                "images": urls,
                "manifest": str(out),
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
