#!/usr/bin/env python3
"""Shelf-pack staged sprites into deterministic PixiJS atlas pages.

Scans ``staged/<group>/`` for ``*.png`` + sibling ``.meta.json``, then per group
shelf-packs the sprites into square RGBA pages of ``--page-size``. For every
page it also emits a PixiJS spritesheet JSON. Everything is deterministic:
sprites are sorted by height desc then key asc, so the same staged dir always
yields byte-identical PNGs and JSON (fixed PNG save params).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
from io import BytesIO
from pathlib import Path
from typing import Any

from PIL import Image


class OversizeError(Exception):
    """A single sprite cannot fit a page even with edge padding."""


def atomic_write_bytes(path: Path, data: bytes) -> None:
    """Write via an adjacent temp file then os.replace so a crash mid-write
    can never leave a half-written atlas page or spritesheet behind."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        os.replace(tmp, path)
    finally:
        try:
            os.remove(tmp)
        except OSError:
            pass


def sanitize(key: str) -> str:
    """Filesystem-safe name for a semantic key: ':' -> '__'."""
    return key.replace(":", "__")


def copy_single(src: Path, dest: Path) -> None:
    """Atomic copy of a standalone (singles) PNG to the atlas dir."""
    atomic_write_bytes(dest, src.read_bytes())


def load_staged(staged_dir: Path) -> list[dict]:
    """Collect every staged sprite as {group,key,w,h,png} from *.png+meta."""
    sprites: list[dict] = []
    if not staged_dir.exists():
        return sprites
    for group_dir in sorted(paged for paged in staged_dir.iterdir() if paged.is_dir()):
        for png in sorted(group_dir.glob("*.png")):
            meta_path = png.with_suffix(".meta.json")
            if not meta_path.exists():
                continue
            meta = json.loads(meta_path.read_text(encoding="utf-8"))
            sprites.append(
                {
                    "group": group_dir.name,
                    "key": meta["key"],
                    "w": int(meta["w"]),
                    "h": int(meta["h"]),
                    "png": png,
                }
            )
    return sprites


def shelf_pack(
    sprites: list[dict], page_size: int, padding: int
) -> list[list[tuple[str, int, int, int, int, Path]]]:
    """Shelf-pack sprites (already sorted height desc, key asc) into pages.

    Each shelf is a row whose height equals its tallest sprite; a new shelf is
    started when the next sprite won't fit the remaining row width, and a new
    page when it won't fit the remaining column height. ``padding`` transparent
    px separate sprites and pad the page edges. A sprite that cannot fit even a
    bare page raises OversizeError naming the key.
    """
    pages: list[list[tuple[str, int, int, int, int, Path]]] = []
    cur: list[tuple[str, int, int, int, int, Path]] = []
    x = padding
    y = padding
    shelf_h = 0

    for s in sprites:
        w, h = s["w"], s["h"]
        if w + 2 * padding > page_size or h + 2 * padding > page_size:
            raise OversizeError(s["key"])
        if x + w + padding > page_size:
            # Start a higher shelf within the same page.
            y += shelf_h + padding
            x = padding
            shelf_h = 0
        if y + h + padding > page_size:
            # Current page cannot hold this sprite's height even on a fresh
            # shelf: begin a new page. Hoisting this out of the "wrap shelf"
            # branch means a future sort change (e.g. not strictly height-desc)
            # can never silently drop a sprite past the page bottom.
            pages.append(cur)
            cur = []
            x = padding
            y = padding
            shelf_h = 0
        cur.append((s["key"], x, y, w, h, s["png"]))
        x += w + padding
        if h > shelf_h:
            shelf_h = h

    if cur:
        pages.append(cur)
    return pages


def write_atlas_json(path: Path, group: str, page_index: int, page_size: int,
                     frames: dict[str, dict]) -> None:
    """Emit a PixiJS spritesheet for one page; frame names are raw semantic keys."""
    ordered = {k: frames[k] for k in sorted(frames)}
    doc = {
        "frames": ordered,
        "meta": {
            "image": f"{group}-{page_index}.png",
            "format": "RGBA8888",
            "size": {"w": page_size, "h": page_size},
            "scale": "1",
        },
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(doc, indent=2) + "\n"
    atomic_write_bytes(path, text.encode("utf-8"))


def pack_group(group: str, sprites: list[dict], page_size: int, padding: int,
              out: Path) -> list[Path]:
    """Render every page PNG/JSON for one group; return the written PNG paths."""
    ordered = sorted(sprites, key=lambda s: (-s["h"], s["key"]))
    pages = shelf_pack(ordered, page_size, padding)
    written: list[Path] = []
    for i, placements in enumerate(pages):
        canvas = Image.new("RGBA", (page_size, page_size), (0, 0, 0, 0))
        frames: dict[str, dict] = {}
        for key, x, y, w, h, png in placements:
            im = Image.open(png).convert("RGBA")
            canvas.paste(im, (x, y))
            frames[key] = {
                "frame": {"x": x, "y": y, "w": w, "h": h},
                "rotated": False,
                "trimmed": False,
                "spriteSourceSize": {"x": 0, "y": 0, "w": w, "h": h},
                "sourceSize": {"w": w, "h": h},
            }
        png_path = out / f"{group}-{i}.png"
        buf = BytesIO()
        canvas.save(buf, "PNG", optimize=False, compress_level=9)
        atomic_write_bytes(png_path, buf.getvalue())
        write_atlas_json(out / f"{group}-{i}.json", group, i, page_size, frames)
        written.append(png_path)
    return written


def print_summary(summary: list[tuple[str, int, int, int, int]], total_pages: int,
                  total_bytes: int) -> None:
    print(f"{'GROUP':<14}{'SPRITES':>8}{'PAGES':>7}{'BYTES':>12}{'SINGLES':>8}")
    total_singles = 0
    for group, sprites, pages, bytes_, singles in summary:
        print(f"{group:<14}{sprites:>8}{pages:>7}{bytes_:>12}{singles:>8}")
        total_singles += singles
    print(f"{'TOTAL':<14}{'':>8}{total_pages:>7}{total_bytes:>12}{total_singles:>8}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Pack staged sprites into atlas pages")
    parser.add_argument("--staged", required=True, type=Path, help="staged dir")
    parser.add_argument("--out", required=True, type=Path, help="atlas output dir")
    parser.add_argument("--page-size", type=int, default=2048)
    parser.add_argument("--padding", type=int, default=2)
    parser.add_argument("--max-pages", type=int, default=6)
    parser.add_argument("--max-bytes", type=int, default=12_000_000)
    parser.add_argument("--singles-groups", default="tex",
                        help="comma-separated groups shipped as standalone "
                             "pow2 PNGs (not shelf-packed)")
    args = parser.parse_args(argv)

    args.out.mkdir(parents=True, exist_ok=True)
    sprites = load_staged(args.staged)
    by_group: dict[str, list[dict]] = {}
    for s in sprites:
        by_group.setdefault(s["group"], []).append(s)

    summary: list[tuple[str, int, int, int, int]] = []
    total_pages = 0
    total_bytes = 0
    singles_groups = {g.strip() for g in args.singles_groups.split(",") if g.strip()}

    for group in sorted(by_group):
        sprites = by_group[group]
        if group in singles_groups:
            # Standalone repeatable textures: copy each PNG as-is (atomically),
            # excluded from page packing. They count toward bytes, not pages.
            written_singles: list[Path] = []
            for s in sprites:
                dest = args.out / f"{sanitize(s['key'])}.png"
                copy_single(s["png"], dest)
                written_singles.append(dest)
            group_bytes = sum(p.stat().st_size for p in written_singles)
            summary.append((group, len(sprites), 0, group_bytes,
                            len(written_singles)))
            total_bytes += group_bytes
            continue
        try:
            written = pack_group(group, sprites, args.page_size,
                                  args.padding, args.out)
        except OversizeError as exc:
            print(f"ERROR oversize sprite: {exc} exceeds page size {args.page_size}",
                  file=sys.stderr)
            return 1
        group_bytes = sum(p.stat().st_size for p in written)
        summary.append((group, len(sprites), len(written), group_bytes, 0))
        total_pages += len(written)
        total_bytes += group_bytes

    if total_pages > args.max_pages or total_bytes > args.max_bytes:
        print("BUDGET EXCEEDED", file=sys.stderr)
        for group, sprites_n, pages, bytes_, singles_n in summary:
            print(f"  {group}: sprites={sprites_n} pages={pages} bytes={bytes_} "
                  f"singles={singles_n}", file=sys.stderr)
        print(f"TOTAL: pages={total_pages} (max {args.max_pages}) "
              f"bytes={total_bytes} (max {args.max_bytes})", file=sys.stderr)
        print("stale files left in --out for inspection; clean before rerun",
              file=sys.stderr)
        return 2

    print_summary(summary, total_pages, total_bytes)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
