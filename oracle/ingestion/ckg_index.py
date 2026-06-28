"""Bridge to the Rust `ckg` CLI for code-knowledge-graph ingestion into the CKG store.

Shells `<app_bin> ckg --root <path>`, parses its JSON ({nodes, edges, capped}), attaches the
source-file of each edge (so the store's delete-by-file delta works), and loads it into CkgStore.
The CKG re-parse is CPU-only — it rides the SAME post-commit/on_batch_ready trigger as the GPU
vector re-index but does NOT contend for the GPU.
"""

import json
import subprocess
from pathlib import Path
from typing import Any

from oracle.config import CKG_DB_PATH
from oracle.store.ckg_store import CkgStore

CKG_BRIDGE_TIMEOUT_S = 120.0
CKG_MAX_OUTPUT_BYTES = 64 * 1024 * 1024


def _run_ckg_bridge(app_bin: str, root: Path) -> dict:
    """Invoke `<app_bin> ckg --root <root>` and parse its stdout JSON ({nodes, edges, capped}).
    Raises RuntimeError on timeout / non-zero exit / unparseable output (never lets a subprocess
    exception escape)."""
    try:
        proc = subprocess.run(
            [app_bin, "ckg", "--root", str(root)],
            capture_output=True,
            timeout=CKG_BRIDGE_TIMEOUT_S,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        raise RuntimeError(f"ckg bridge timed out after {CKG_BRIDGE_TIMEOUT_S:.0f}s") from exc
    except OSError as exc:
        raise RuntimeError(f"ckg bridge could not run: {exc}") from exc
    if proc.returncode != 0:
        detail = (proc.stderr or b"").decode("utf-8", "replace").strip()
        detail = detail.splitlines()[0][:200] if detail else "no diagnostic"
        raise RuntimeError(f"ckg bridge failed (exit {proc.returncode}): {detail}")
    raw = proc.stdout or b""
    if len(raw) > CKG_MAX_OUTPUT_BYTES:
        raise RuntimeError("ckg graph output exceeded the size limit.")
    try:
        graph = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError) as exc:
        raise RuntimeError("ckg bridge returned unparseable JSON.") from exc
    if not isinstance(graph, dict):
        raise RuntimeError("ckg bridge returned a non-object graph.")
    return graph


def _attach_src_file(graph: dict) -> tuple[list[dict], list[dict]]:
    """Attach `srcFile` to each edge, resolved from the src node's `file` (falling back to the
    edge's `src` id — for a CONTAIN edge `src` IS the file id). The Rust bridge emits camelCase
    node dicts {id,kind,name,file,startLine,endLine,lang} and edge dicts {src,dst,kind}; the
    CkgStore insert wants exactly those keys plus `srcFile` on edges, so no renaming is needed."""
    nodes: list[dict] = graph.get("nodes", []) or []
    edges: list[dict] = graph.get("edges", []) or []

    node_file: dict[str, str] = {}
    for n in nodes:
        nid = n.get("id")
        if nid is not None:
            node_file[nid] = n.get("file", nid)

    for edge in edges:
        src_id = edge.get("src", "")
        edge["srcFile"] = node_file.get(src_id, src_id)

    return (nodes, edges)


def build_ckg(
    root: Path,
    app_bin: str,
    *,
    runner: Any | None = None,
    store: CkgStore | None = None,
) -> dict:
    """Run the `ckg` bridge over `root` and load the whole graph into the store (full rebuild).
    `runner` is an injectable `(app_bin, root) -> dict` for tests (no real subprocess)."""
    graph = (runner or _run_ckg_bridge)(app_bin, root)
    nodes, edges = _attach_src_file(graph)

    s = store or CkgStore(CKG_DB_PATH)
    s.replace_all(nodes, edges)

    return {
        "nodeCount": len(nodes),
        "edgeCount": len(edges),
        "capped": bool(graph.get("capped")),
    }


def update_ckg_for_files(
    root: Path,
    changed_files: list[str],
    app_bin: str,
    *,
    runner: Any | None = None,
    store: CkgStore | None = None,
) -> dict:
    """Refresh the CKG after a file change. The Rust bridge currently re-walks the WHOLE tree
    (no `--files` flag yet), so this is a full rebuild. A future `--files` bridge flag would let
    this do a true delta via `store.replace_for_files(changed_files, nodes, edges)`."""
    return build_ckg(root, app_bin, runner=runner, store=store)


def test_build_ckg_attaches_src_file_and_loads(tmp_path: Path) -> None:
    def fake_runner(app_bin: str, root: Path) -> dict:
        return {
            "nodes": [
                {"id": "a.py", "kind": "FILE", "name": None, "file": "a.py",
                 "startLine": 1, "endLine": 5, "lang": "Python"},
                {"id": "a.py#2-3-0", "kind": "function_definition", "name": "foo", "file": "a.py",
                 "startLine": 2, "endLine": 3, "lang": "Python"},
            ],
            "edges": [{"src": "a.py", "dst": "a.py#2-3-0", "kind": "CONTAIN"}],
            "capped": False,
        }

    store = CkgStore(tmp_path / "ckg.sqlite")
    res = build_ckg(tmp_path, "fake-bin", runner=fake_runner, store=store)

    assert res["nodeCount"] == 2
    assert res["edgeCount"] == 1
    assert res["capped"] is False

    nbr = store.get_neighborhood("a.py", 1)
    assert len(nbr) >= 1
    assert nbr[0]["id"] == "a.py#2-3-0"
    assert store.find_imports("a.py") == []
