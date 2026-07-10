#!/usr/bin/env python3
"""Offline sprite normalizer for the Polis renderer.

Reads a batch spec (list of jobs) describing raw open-licensed art and emits a
deterministic, trimmed/scaled/recolored copy per sprite into
``tools/polis-art/staged/<group>/`` together with a sidecar ``.meta.json``.

The staged directory is the currency handed to ``pack_atlas.py``: every output
is reproducible (same inputs => byte-identical files), which is what lets the
rest of the pipeline stay cache-friendly and reviewable in git.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tempfile
from io import BytesIO
from pathlib import Path
from typing import Any

from PIL import Image

DEFAULT_ANCHOR = [0.5, 1.0]
# Semantic keys carry only lowercased word chars, ':', '.' and '-'. No slashes,
# so group/name segments can never escape staged/<group>/ via '..' or '/'.
KEY_RE = re.compile(r"^[a-z0-9_.:-]+$")


def stage_root(repo_root: Path) -> Path:
    """Staged art always lives at <root>/tools/polis-art/staged, mirroring raw."""
    return repo_root / "tools" / "polis-art" / "staged"


def group_of(key: str) -> str:
    """The GROUP of a semantic key is its first ':'-separated segment."""
    return key.split(":", 1)[0]


def sanitize(key: str) -> str:
    """Filesystem-safe name: ':' -> '__' so keys round-trip off disk cleanly."""
    return key.replace(":", "__")


def resolve_anchor(anchor: Any) -> list[float]:
    """'auto' means bottom-center of the final image => normalized [0.5, 1.0].

    An explicit [ax, ay] pair of normalized floats is passed through untouched.
    """
    if anchor == "auto":
        return [0.5, 1.0]
    if isinstance(anchor, (list, tuple)) and len(anchor) == 2:
        return [float(anchor[0]), float(anchor[1])]
    raise ValueError(f"anchor must be 'auto' or [ax, ay], got {anchor!r}")


def apply_recolor(img: Image.Image, spec: dict) -> Image.Image:
    """Recolor in PIL-native HSV so we never need numpy.

    We shift H by ``hue`` degrees (wrapping 0..255) and scale S/V by ``sat`` /
    ``val``, then merge back the UNAFFECTED original alpha. Keeping the source
    alpha bit-exact is what preserves soft edges and baked shadows.
    """
    if img.mode != "RGBA":
        img = img.convert("RGBA")
    r, g, b, a = img.split()
    hsv = Image.merge("RGB", (r, g, b)).convert("HSV")
    h, s, v = hsv.split()

    hue = float(spec.get("hue", 0.0))
    sat = float(spec.get("sat", 1.0))
    val = float(spec.get("val", 1.0))

    h_shift = (hue / 360.0) * 255.0
    h = h.point(lambda x: int((x + h_shift) % 256))
    s = s.point(lambda x: max(0, min(255, int(x * sat))))
    v = v.point(lambda x: max(0, min(255, int(x * val))))

    rgb2 = Image.merge("HSV", (h, s, v)).convert("RGB")
    r2, g2, b2 = rgb2.split()
    return Image.merge("RGBA", (r2, g2, b2, a))


def atomic_write(path: Path, data: bytes) -> None:
    """Write via an adjacent temp file then os.replace so a crash mid-write
    can never leave a half-written artifact in the staged dir."""
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


def process_job(job: dict, repo_root: Path, staged: Path) -> None:
    """Normalize one spec job fully in memory, then atomically persist it.

    Raises on malformed input; the caller translates that into a SKIP so one
    bad sprite never aborts the whole batch.
    """
    key = job["key"]
    if key is None or not KEY_RE.match(key):
        raise ValueError(
            f"invalid key {key!r} (must match ^[a-z0-9_.:-]+$)"
        )
    source = job["source"]
    in_rel = job["in"]
    scale = float(job.get("scale", 1.0))
    do_trim = bool(job.get("trim", True))
    recolor = job.get("recolor")
    foot = job.get("foot")  # None when absent
    has_baked = bool(job.get("hasBakedShadow", False))
    anchor_out = resolve_anchor(job.get("anchor", "auto"))

    src = repo_root / in_rel
    if not src.exists():
        raise FileNotFoundError(f"input not found: {in_rel}")
    img = Image.open(src).convert("RGBA")
    if img.getbbox() is None:
        raise ValueError(f"fully transparent image: {in_rel}")

    # Optional [x, y, w, h] crop applied FIRST — this is how flip-book frames
    # are cut out of a sprite strip/grid. Bounds-checked so a bad spec fails
    # the job loudly instead of silently wrapping.
    crop = job.get("crop")
    if crop is not None:
        if not (isinstance(crop, (list, tuple)) and len(crop) == 4):
            raise ValueError(f"crop must be [x, y, w, h], got {crop!r}")
        x, y, w, h = (int(v) for v in crop)
        if w <= 0 or h <= 0 or x < 0 or y < 0 or x + w > img.width or y + h > img.height:
            raise ValueError(f"crop {crop} outside image {img.width}x{img.height}")
        img = img.crop((x, y, x + w, y + h))
        if img.getbbox() is None:
            raise ValueError(f"crop {crop} is fully transparent: {in_rel}")

    if do_trim:
        bbox = img.getbbox()
        if bbox is not None:
            img = img.crop(bbox)

    if scale != 1.0:
        nw = max(1, round(img.width * scale))
        nh = max(1, round(img.height * scale))
        img = img.resize((nw, nh), Image.LANCZOS)

    if recolor:
        img = apply_recolor(img, recolor)

    w, h = img.width, img.height

    group = group_of(key)
    out_png = staged / group / f"{sanitize(key)}.png"
    out_meta = staged / group / f"{sanitize(key)}.meta.json"

    buf = BytesIO()
    img.save(buf, "PNG")
    atomic_write(out_png, buf.getvalue())

    meta: dict[str, Any] = {
        "key": key,
        "source": source,
        "in": in_rel,
        "scale": scale,
        "anchor": anchor_out,
    }
    if crop is not None:
        meta["crop"] = [int(v) for v in crop]
    if foot is not None:
        meta["foot"] = list(foot)
    meta["hasBakedShadow"] = has_baked
    meta["w"] = w
    meta["h"] = h
    meta_str = json.dumps(meta, indent=2, sort_keys=True) + "\n"
    atomic_write(out_meta, meta_str.encode("utf-8"))


def run(spec_path: Path, repo_root: Path) -> int:
    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    jobs = spec.get("jobs", [])
    staged = stage_root(repo_root)
    skipped: list[str] = []

    for job in jobs:
        key = job.get("key", "<unknown>")
        try:
            if "key" not in job or "source" not in job or "in" not in job:
                raise ValueError("job requires key, source and in")
            process_job(job, repo_root, staged)
        except Exception as exc:  # noqa: BLE001 - batch continues on any failure
            reason = f"{type(exc).__name__}: {exc}"
            print(f"SKIP {key}: {reason}", file=sys.stderr)
            skipped.append(key)

    return 1 if skipped else 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Normalize raw sprites into staged/")
    parser.add_argument("--spec", required=True, type=Path, help="batch spec JSON")
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="repo root")
    args = parser.parse_args(argv)
    return run(args.spec, args.root.resolve())


if __name__ == "__main__":
    raise SystemExit(main())
