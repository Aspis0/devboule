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
import sys
from pathlib import Path
from typing import Any

from PIL import Image


class OversizeError(Exception):
    """A single sprite cannot fit a page even with edge padding."""


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
                # Current page is full; begin a fresh one.
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
    with open(path, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")


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
        canvas.save(png_path, "PNG", optimize=False, compress_level=9)
        write_atlas_json(out / f"{group}-{i}.json", group, i, page_size, frames)
        written.append(png_path)
    return written


def print_summary(summary: list[tuple[str, int, int, int]], total_pages: int,
                  total_bytes: int) -> None:
    print(f"{'GROUP':<14}{'SPRITES':>8}{'PAGES':>7}{'BYTES':>12}")
    for group, sprites, pages, bytes_ in summary:
        print(f"{group:<14}{sprites:>8}{pages:>7}{bytes_:>12}")
    print(f"{'TOTAL':<14}{'':>8}{total_pages:>7}{total_bytes:>12}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Pack staged sprites into atlas pages")
    parser.add_argument("--staged", required=True, type=Path, help="staged dir")
    parser.add_argument("--out", required=True, type=Path, help="atlas output dir")
    parser.add_argument("--page-size", type=int, default=2048)
    parser.add_argument("--padding", type=int, default=2)
    parser.add_argument("--max-pages", type=int, default=6)
    parser.add_argument("--max-bytes", type=int, default=12_000_000)
    args = parser.parse_args(argv)

    args.out.mkdir(parents=True, exist_ok=True)
    sprites = load_staged(args.staged)
    by_group: dict[str, list[dict]] = {}
    for s in sprites:
        by_group.setdefault(s["group"], []).append(s)

    summary: list[tuple[str, int, int, int]] = []
    total_pages = 0
    total_bytes = 0

    for group in sorted(by_group):
        try:
            written = pack_group(group, by_group[group], args.page_size,
                                  args.padding, args.out)
        except OversizeError as exc:
            print(f"ERROR oversize sprite: {exc} exceeds page size {args.page_size}",
                  file=sys.stderr)
            return 1
        group_bytes = sum(p.stat().st_size for p in written)
        summary.append((group, len(by_group[group]), len(written), group_bytes))
        total_pages += len(written)
        total_bytes += group_bytes

    if total_pages > args.max_pages or total_bytes > args.max_bytes:
        print("BUDGET EXCEEDED", file=sys.stderr)
        for group, sprites_n, pages, bytes_ in summary:
            print(f"  {group}: sprites={sprites_n} pages={pages} bytes={bytes_}",
                  file=sys.stderr)
        print(f"TOTAL: pages={total_pages} (max {args.max_pages}) "
              f"bytes={total_bytes} (max {args.max_bytes})", file=sys.stderr)
        return 2

    print_summary(summary, total_pages, total_bytes)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
