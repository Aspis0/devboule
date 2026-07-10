#!/usr/bin/env python3
"""Self-contained end-to-end tests for the Polis sprite-art pipeline.

Builds tiny RGBA fixtures with PIL into tmp_path, then exercises normalize ->
pack -> manifest through their module-level ``main()`` so the whole offline
pipeline is verified without touching the repo's real assets.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest
from PIL import Image

# Make the sibling pipeline scripts importable.
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

import normalize  # noqa: E402
import pack_atlas  # noqa: E402
import manifest  # noqa: E402

POLIS_ART = ROOT / "tools" / "polis-art"


def make_raw(root: Path, name: str, size: int, color: tuple) -> str:
    """Write an opaque RGBA sprite under <root>/tools/polis-art/raw and return
    the spec-relative ``in`` path used by normalize."""
    raw = POLIS_ART_under(root) / "raw"
    path = raw / name
    path.parent.mkdir(parents=True, exist_ok=True)
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    px = img.load()
    for y in range(size):
        for x in range(size):
            px[x, y] = color
    img.save(path)
    return f"tools/polis-art/raw/{name}"


def POLIS_ART_under(root: Path) -> Path:
    return root / "tools" / "polis-art"


def staged_dir(root: Path) -> Path:
    return POLIS_ART_under(root) / "staged"


def make_trim_fixture(root: Path) -> str:
    """8x8 sprite, 2px transparent border, opaque 4x4 center (bbox 4x4)."""
    raw = POLIS_ART_under(root) / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    path = raw / "trim8.png"
    img = Image.new("RGBA", (8, 8), (0, 0, 0, 0))
    px = img.load()
    for y in range(2, 6):
        for x in range(2, 6):
            px[x, y] = (200, 100, 50, 255)
    img.save(path)
    return "tools/polis-art/raw/trim8.png"


def write_spec(path: Path, jobs: list[dict]) -> Path:
    path.write_text(json.dumps({"jobs": jobs}, indent=2))
    return path


# --------------------------------------------------------------------------- #
# normalize
# --------------------------------------------------------------------------- #
def test_trim_scale(tmp_path):
    in_rel = make_trim_fixture(tmp_path)
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "prop:trim:v0", "source": "s",
                       "in": in_rel, "scale": 2.0}])
    rc = normalize.main(["--spec", str(spec), "--root", str(tmp_path)])
    assert rc == 0

    out_png = staged_dir(tmp_path) / "prop" / "prop__trim__v0.png"
    out_meta = staged_dir(tmp_path) / "prop" / "prop__trim__v0.meta.json"
    assert out_png.exists() and out_meta.exists()

    img = Image.open(out_png)
    assert img.size == (8, 8)  # bbox 4x4 * scale 2
    meta = json.loads(out_meta.read_text())
    assert meta["w"] == 8 and meta["h"] == 8
    assert meta["anchor"] == [0.5, 1.0]


def test_recolor_hue_rotate(tmp_path):
    in_rel = make_raw(tmp_path, "red4.png", 4, (255, 0, 0, 255))
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "prop:red:v0", "source": "s", "in": in_rel,
                       "recolor": {"hue": 120}}])
    rc = normalize.main(["--spec", str(spec), "--root", str(tmp_path)])
    assert rc == 0

    out_png = staged_dir(tmp_path) / "prop" / "prop__red__v0.png"
    img = Image.open(out_png).convert("RGBA")
    r, g, b, a = img.getpixel((0, 0))
    assert abs(r - 0) <= 2
    assert abs(g - 255) <= 2
    assert abs(b - 0) <= 2
    assert a == 255  # alpha untouched


def test_skip_missing_input(tmp_path):
    good = make_trim_fixture(tmp_path)
    spec = tmp_path / "spec.json"
    write_spec(spec, [
        {"key": "prop:good:v0", "source": "s", "in": good},
        {"key": "prop:bad:v0", "source": "s", "in": "tools/polis-art/raw/nope.png"},
    ])
    rc = normalize.main(["--spec", str(spec), "--root", str(tmp_path)])
    assert rc == 1  # any skip => exit 1
    assert (staged_dir(tmp_path) / "prop" / "prop__good__v0.png").exists()
    assert not (staged_dir(tmp_path) / "prop" / "prop__bad__v0.png").exists()


# --------------------------------------------------------------------------- #
# pack_atlas
# --------------------------------------------------------------------------- #
def _stage_three(tmp_path):
    sizes = {"big": 14, "med": 10, "small": 6}
    jobs = []
    for kind, sz in sizes.items():
        in_rel = make_raw(tmp_path, f"{kind}.png", sz, (10, 20, 30, 255))
        jobs.append({"key": f"prop:{kind}:v0", "source": "s", "in": in_rel})
    spec = tmp_path / "spec.json"
    write_spec(spec, jobs)
    assert normalize.main(["--spec", str(spec), "--root", str(tmp_path)]) == 0
    return staged_dir(tmp_path)


def test_pack_single_page_deterministic(tmp_path):
    staged = _stage_three(tmp_path)
    out1 = tmp_path / "atlas1"
    out2 = tmp_path / "atlas2"
    assert pack_atlas.main(["--staged", str(staged), "--out", str(out1)]) == 0
    assert pack_atlas.main(["--staged", str(staged), "--out", str(out2)]) == 0

    png1 = out1 / "prop-0.png"
    png2 = out2 / "prop-0.png"
    json1 = out1 / "prop-0.json"
    json2 = out2 / "prop-0.json"
    assert png1.exists() and json1.exists()
    assert png2.exists() and json2.exists()
    assert png1.read_bytes() == png2.read_bytes()
    assert json1.read_bytes() == json2.read_bytes()

    data = json.loads(json1.read_text())
    assert set(data["frames"]) == {"prop:big:v0", "prop:med:v0", "prop:small:v0"}
    assert data["frames"]["prop:big:v0"]["frame"]["w"] == 14

    # rects must not overlap and stay inside the page.
    rects = [f["frame"] for f in data["frames"].values()]
    for i in range(len(rects)):
        for j in range(i + 1, len(rects)):
            a, b = rects[i], rects[j]
            no_overlap = (
                a["x"] + a["w"] <= b["x"] or b["x"] + b["w"] <= a["x"]
                or a["y"] + a["h"] <= b["y"] or b["y"] + b["h"] <= a["y"]
            )
            assert no_overlap, (a, b)
    for r in rects:
        assert r["x"] >= 0 and r["y"] >= 0
        assert r["x"] + r["w"] <= data["meta"]["size"]["w"]
        assert r["y"] + r["h"] <= data["meta"]["size"]["h"]


def test_budget_exceeded(tmp_path, capfd):
    staged = _stage_three(tmp_path)
    rc = pack_atlas.main(["--staged", str(staged), "--out",
                          str(tmp_path / "atlas"), "--max-bytes", "1"])
    assert rc == 2
    assert "BUDGET EXCEEDED" in capfd.readouterr().err


def test_oversize_sprite(tmp_path, capfd):
    staged = staged_dir(tmp_path)
    group = staged / "prop"
    group.mkdir(parents=True, exist_ok=True)
    png = group / "prop__big__v0.png"
    Image.new("RGBA", (20, 20), (1, 2, 3, 255)).save(png)
    (group / "prop__big__v0.meta.json").write_text(json.dumps(
        {"key": "prop:big:v0", "source": "s", "in": "x", "scale": 1.0,
         "anchor": [0.5, 1.0], "hasBakedShadow": False, "w": 20, "h": 20}))

    rc = pack_atlas.main(["--staged", str(staged), "--out",
                          str(tmp_path / "atlas"), "--page-size", "8",
                          "--padding", "1"])
    assert rc == 1
    err = capfd.readouterr().err
    assert "prop:big:v0" in err


# --------------------------------------------------------------------------- #
# manifest (end-to-end)
# --------------------------------------------------------------------------- #
def _mini_ledger(path: Path, sources: dict):
    path.write_text(json.dumps(
        {"version": 1, "sources": sources, "assets": []}, indent=2))


def test_manifest_end_to_end(tmp_path):
    in_rel = make_raw(tmp_path, "olive.png", 12, (0, 128, 0, 255))
    spec = tmp_path / "spec.json"
    write_spec(spec, [{
        "key": "prop:olive:v0", "source": "test-src", "in": in_rel,
        "foot": [1, 1], "hasBakedShadow": True,
    }])
    assert normalize.main(["--spec", str(spec), "--root", str(tmp_path)]) == 0
    staged = staged_dir(tmp_path)
    atlas = tmp_path / "atlas"
    assert pack_atlas.main(["--staged", str(staged), "--out", str(atlas)]) == 0

    ledger = tmp_path / "ledger.json"
    _mini_ledger(ledger, {"test-src": {"license": "CC0"}})

    out_ts = tmp_path / "spriteManifest.ts"
    rc = manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                        "--ledger", str(ledger), "--out", str(out_ts)])
    assert rc == 0
    ts = out_ts.read_text()
    assert '"prop:olive:v0"' in ts
    assert "foot: [1, 1]" in ts
    assert "hasBakedShadow: true" in ts
    assert "anchor:" not in ts  # default anchor omitted
    assert "GENERATED by tools/polis-art/manifest.py" in ts

    # Missing source in ledger => validation failure, no output written.
    bad_ledger = tmp_path / "ledger_bad.json"
    _mini_ledger(bad_ledger, {})
    out_ts.unlink()
    rc2 = manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                         "--ledger", str(bad_ledger), "--out", str(out_ts)])
    assert rc2 == 1
    assert not out_ts.exists()


# --------------------------------------------------------------------------- #
# hostile-review regression tests
# --------------------------------------------------------------------------- #

def test_normalize_rejects_path_traversal_key(tmp_path):
    in_rel = make_trim_fixture(tmp_path)
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "../../etc:x", "source": "s", "in": in_rel}])
    rc = normalize.main(["--spec", str(spec), "--root", str(tmp_path)])
    assert rc == 1  # SKIP path -> exit 1
    staged = staged_dir(tmp_path)
    # Nothing should be written, and the '..' must not escape staged/.
    assert list(staged.rglob("*.png")) == []
    assert not (staged / "etc").exists()


def test_normalize_skips_fully_transparent(tmp_path):
    raw = POLIS_ART_under(tmp_path) / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    Image.new("RGBA", (8, 8), (0, 0, 0, 0)).save(raw / "empty.png")
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "prop:empty:v0", "source": "s",
                       "in": "tools/polis-art/raw/empty.png"}])
    rc = normalize.main(["--spec", str(spec), "--root", str(tmp_path)])
    assert rc == 1
    assert list(staged_dir(tmp_path).rglob("*.png")) == []


def test_downscale_size_math(tmp_path):
    in_rel = make_trim_fixture(tmp_path)  # 4x4 non-transparent bbox
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "prop:t:v0", "source": "s", "in": in_rel,
                       "scale": 0.5}])
    assert normalize.main(["--spec", str(spec), "--root", str(tmp_path)]) == 0
    out_png = staged_dir(tmp_path) / "prop" / "prop__t__v0.png"
    img = Image.open(out_png)
    assert img.size == (2, 2)  # round(4 * 0.5)
    meta = json.loads(
        (staged_dir(tmp_path) / "prop" / "prop__t__v0.meta.json").read_text())
    assert meta["w"] == 2 and meta["h"] == 2


def test_shelf_pack_out_of_order_does_not_overflow():
    """shelf_pack must catch a tall sprite arriving late, even when the global
    height-desc sort is bypassed by calling it directly with unsorted input."""
    sprites = [
        {"key": "a", "w": 5, "h": 10, "png": Path("/n/a.png")},
        {"key": "b", "w": 5, "h": 3, "png": Path("/n/b.png")},
        {"key": "e", "w": 5, "h": 3, "png": Path("/n/e.png")},
        {"key": "c", "w": 3, "h": 15, "png": Path("/n/c.png")},  # tall, last
    ]
    pages = pack_atlas.shelf_pack(sprites, page_size=20, padding=2)
    for page in pages:
        for _key, x, y, w, h, _png in page:
            assert y + h + 2 <= 20, "sprite placed past page bottom"
    # The guard must have forced the tall sprite onto a new page.
    assert len(pages) == 2


def test_manifest_rejects_malformed_meta(tmp_path, capfd):
    staged = staged_dir(tmp_path)
    g = staged / "prop"
    g.mkdir(parents=True, exist_ok=True)
    (g / "prop__a__v0.meta.json").write_text(json.dumps({"key": "prop:a:v0"}))
    (g / "prop__b__v0.meta.json").write_text(json.dumps({"source": "s"}))
    ledger = tmp_path / "ledger.json"
    ledger.write_text(json.dumps({"version": 1, "sources": {"s": {}}, "assets": []}))
    out = tmp_path / "out.ts"
    rc = manifest.main(["--staged", str(staged),
                        "--atlas-dir", str(tmp_path / "atlas"),
                        "--ledger", str(ledger), "--out", str(out)])
    assert rc == 1  # validation error, not a KeyError crash
    err = capfd.readouterr().err
    assert "missing field" in err
    assert not out.exists()


def test_manifest_rejects_hostile_key(tmp_path):
    staged = staged_dir(tmp_path)
    g = staged / "prop"
    g.mkdir(parents=True, exist_ok=True)
    (g / "prop__x.meta.json").write_text(json.dumps(
        {"key": 'prop"x:v0', "source": "s", "w": 4, "h": 4}))
    ledger = tmp_path / "ledger.json"
    ledger.write_text(json.dumps({"version": 1, "sources": {"s": {}}, "assets": []}))
    out = tmp_path / "out.ts"
    rc = manifest.main(["--staged", str(staged),
                        "--atlas-dir", str(tmp_path / "atlas"),
                        "--ledger", str(ledger), "--out", str(out)])
    assert rc == 1
    assert not out.exists()  # validation caught it before any (corrupt) emit


def test_manifest_empty_staged_emits_empty(tmp_path):
    staged = staged_dir(tmp_path)
    staged.mkdir(parents=True, exist_ok=True)
    atlas = tmp_path / "atlas"
    atlas.mkdir(parents=True, exist_ok=True)
    ledger = tmp_path / "ledger.json"
    ledger.write_text(json.dumps({"version": 1, "sources": {"s": {}}, "assets": []}))
    out = tmp_path / "out.ts"
    rc = manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                        "--ledger", str(ledger), "--out", str(out)])
    assert rc == 0
    ts = out.read_text()
    assert "entries: {}" in ts
    assert "atlases: {}" in ts


def test_manifest_deterministic_rerun(tmp_path):
    in_rel = make_raw(tmp_path, "o.png", 12, (0, 128, 0, 255))
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "prop:o:v0", "source": "s", "in": in_rel}])
    assert normalize.main(["--spec", str(spec), "--root", str(tmp_path)]) == 0
    staged = staged_dir(tmp_path)
    atlas = tmp_path / "atlas"
    assert pack_atlas.main(["--staged", str(staged), "--out", str(atlas)]) == 0
    ledger = tmp_path / "ledger.json"
    ledger.write_text(json.dumps({"version": 1, "sources": {"s": {}}, "assets": []}))
    out1 = tmp_path / "out1.ts"
    out2 = tmp_path / "out2.ts"
    assert manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                          "--ledger", str(ledger), "--out", str(out1)]) == 0
    assert manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                          "--ledger", str(ledger), "--out", str(out2)]) == 0
    assert out1.read_bytes() == out2.read_bytes()


def test_manifest_custom_anchor_propagates(tmp_path):
    in_rel = make_raw(tmp_path, "o.png", 12, (0, 128, 0, 255))
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "prop:o:v0", "source": "s", "in": in_rel,
                       "anchor": [0.5, 0.3]}])
    assert normalize.main(["--spec", str(spec), "--root", str(tmp_path)]) == 0
    staged = staged_dir(tmp_path)
    atlas = tmp_path / "atlas"
    assert pack_atlas.main(["--staged", str(staged), "--out", str(atlas)]) == 0
    ledger = tmp_path / "ledger.json"
    ledger.write_text(json.dumps({"version": 1, "sources": {"s": {}}, "assets": []}))
    out = tmp_path / "out.ts"
    assert manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                          "--ledger", str(ledger), "--out", str(out)]) == 0
    ts = out.read_text()
    assert '"prop:o:v0"' in ts
    assert "anchor: [0.5, 0.3]" in ts


# --------------------------------------------------------------------------- #
# "singles" (standalone repeatable textures) regression tests
# --------------------------------------------------------------------------- #

def test_pack_singles_copied_excluded_from_pages(tmp_path):
    in_tex = make_raw(tmp_path, "grass.png", 16, (0, 200, 0, 255))
    in_prop = make_raw(tmp_path, "rock.png", 10, (120, 0, 0, 255))
    spec = tmp_path / "spec.json"
    write_spec(spec, [
        {"key": "tex:grass", "source": "s", "in": in_tex},
        {"key": "prop:rock:v0", "source": "s", "in": in_prop},
    ])
    assert normalize.main(["--spec", str(spec), "--root", str(tmp_path)]) == 0
    staged = staged_dir(tmp_path)
    out1 = tmp_path / "atlas1"
    assert pack_atlas.main(["--staged", str(staged), "--out", str(out1)]) == 0

    single_png = out1 / "tex__grass.png"
    assert single_png.exists()
    # Standalone copy is byte-identical to the staged source PNG.
    staged_png = staged / "tex" / "tex__grass.png"
    assert single_png.read_bytes() == staged_png.read_bytes()
    # tex is excluded from page packing (no tex-0 page).
    assert not (out1 / "tex-0.png").exists()
    assert not (out1 / "tex-0.json").exists()
    # The normal group still shelves into a page.
    assert (out1 / "prop-0.png").exists() and (out1 / "prop-0.json").exists()

    # Byte-identical on a second run.
    out2 = tmp_path / "atlas2"
    assert pack_atlas.main(["--staged", str(staged), "--out", str(out2)]) == 0
    assert (out2 / "tex__grass.png").read_bytes() == single_png.read_bytes()


def test_pack_singles_counted_in_bytes(tmp_path):
    in_tex = make_raw(tmp_path, "grass.png", 16, (0, 200, 0, 255))
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "tex:grass", "source": "s", "in": in_tex}])
    assert normalize.main(["--spec", str(spec), "--root", str(tmp_path)]) == 0
    staged = staged_dir(tmp_path)
    size = (staged / "tex" / "tex__grass.png").stat().st_size
    # Singles count toward --max-bytes.
    assert pack_atlas.main(["--staged", str(staged), "--out", str(tmp_path / "a"),
                            "--max-bytes", str(size - 1)]) == 2
    assert pack_atlas.main(["--staged", str(staged), "--out", str(tmp_path / "b"),
                            "--max-bytes", str(size + 1)]) == 0


def test_manifest_emits_singles_and_injection_idempotent(tmp_path):
    in_tex = make_raw(tmp_path, "grass.png", 16, (0, 200, 0, 255))
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "tex:grass", "source": "s", "in": in_tex}])
    assert normalize.main(["--spec", str(spec), "--root", str(tmp_path)]) == 0
    staged = staged_dir(tmp_path)
    atlas = tmp_path / "atlas"
    assert pack_atlas.main(["--staged", str(staged), "--out", str(atlas)]) == 0
    ledger = tmp_path / "ledger.json"
    ledger.write_text(json.dumps({"version": 1, "sources": {"s": {}}, "assets": []}))
    out = tmp_path / "out.ts"
    assert manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                          "--ledger", str(ledger), "--out", str(out)]) == 0
    ts = out.read_text()
    assert '"tex:grass": "/polis/atlas/tex__grass.png"' in ts
    assert "readonly singles?: Record<string, string>;" in ts
    # Interface injection is idempotent (no duplicate members on a 2nd pass).
    once = manifest.inject_singles_interface(ts)
    assert once == ts
    assert ts.count("singles?:") == 1
    # Full re-run yields byte-identical output.
    out2 = tmp_path / "out2.ts"
    assert manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                          "--ledger", str(ledger), "--out", str(out2)]) == 0
    assert out2.read_bytes() == out.read_bytes()


def test_manifest_singles_png_missing_error(tmp_path):
    staged = staged_dir(tmp_path)
    g = staged / "tex"
    g.mkdir(parents=True, exist_ok=True)
    (g / "tex__grass.meta.json").write_text(json.dumps(
        {"key": "tex:grass", "source": "s", "w": 16, "h": 16}))
    atlas = tmp_path / "atlas"
    atlas.mkdir()  # intentionally no tex__grass.png here
    ledger = tmp_path / "ledger.json"
    ledger.write_text(json.dumps({"version": 1, "sources": {"s": {}}, "assets": []}))
    out = tmp_path / "out.ts"
    rc = manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                        "--ledger", str(ledger), "--out", str(out)])
    assert rc == 1
    assert not out.exists()


def test_manifest_dual_presence_error(tmp_path):
    staged = staged_dir(tmp_path)
    g = staged / "tex"
    g.mkdir(parents=True, exist_ok=True)
    (g / "tex__grass.meta.json").write_text(json.dumps(
        {"key": "tex:grass", "source": "s", "w": 4, "h": 4}))
    atlas = tmp_path / "atlas"
    atlas.mkdir(parents=True, exist_ok=True)
    # Atlas frame + standalone PNG for the SAME key => dual presence.
    (atlas / "prop-0.json").write_text(json.dumps({
        "frames": {"tex:grass": {"frame": {"x": 0, "y": 0, "w": 4, "h": 4},
                                  "rotated": False, "trimmed": False,
                                  "spriteSourceSize": {"x": 0, "y": 0, "w": 4, "h": 4},
                                  "sourceSize": {"w": 4, "h": 4}}},
        "meta": {"image": "prop-0.png", "format": "RGBA8888",
                 "size": {"w": 8, "h": 8}, "scale": "1"}}))
    Image.new("RGBA", (8, 8), (0, 0, 0, 0)).save(atlas / "prop-0.png")
    Image.new("RGBA", (4, 4), (0, 128, 0, 255)).save(atlas / "tex__grass.png")
    ledger = tmp_path / "ledger.json"
    ledger.write_text(json.dumps({"version": 1, "sources": {"s": {}}, "assets": []}))
    out = tmp_path / "out.ts"
    rc = manifest.main(["--staged", str(staged), "--atlas-dir", str(atlas),
                        "--ledger", str(ledger), "--out", str(out)])
    assert rc == 1
    assert not out.exists()


# --------------------------------------------------------------------------- #
# crop (A7 — flip-book frames cut from sprite strips/grids)
# --------------------------------------------------------------------------- #
def test_crop_cuts_grid_cell(tmp_path):
    """A 3x3 grid of 4px cells: crop [4,4,4,4] isolates the center cell."""
    raw = POLIS_ART_under(tmp_path) / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    img = Image.new("RGBA", (12, 12), (10, 10, 10, 255))
    px = img.load()
    for y in range(4, 8):
        for x in range(4, 8):
            px[x, y] = (250, 60, 0, 255)
    img.save(raw / "grid.png")
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "fx:fire:f0", "source": "s",
                       "in": "tools/polis-art/raw/grid.png",
                       "trim": False, "crop": [4, 4, 4, 4]}])
    rc = normalize.main(["--spec", str(spec), "--root", str(tmp_path)])
    assert rc == 0
    out_png = staged_dir(tmp_path) / "fx" / "fx__fire__f0.png"
    out = Image.open(out_png)
    assert out.size == (4, 4)
    assert out.getpixel((0, 0)) == (250, 60, 0, 255)
    meta = json.loads((staged_dir(tmp_path) / "fx" / "fx__fire__f0.meta.json").read_text())
    assert meta["crop"] == [4, 4, 4, 4]


def test_crop_out_of_bounds_skips_job(tmp_path):
    in_rel = make_raw(tmp_path, "small4.png", 4, (10, 10, 10, 255))
    spec = tmp_path / "spec.json"
    write_spec(spec, [{"key": "fx:fire:f0", "source": "s", "in": in_rel,
                       "trim": False, "crop": [2, 2, 4, 4]}])
    rc = normalize.main(["--spec", str(spec), "--root", str(tmp_path)])
    assert rc == 1  # SKIP => nonzero, batch continues
    assert not (staged_dir(tmp_path) / "fx" / "fx__fire__f0.png").exists()
