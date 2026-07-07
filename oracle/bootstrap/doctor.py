"""Oracle doctor — the single source of truth for "is Oracle healthy?".

Runs five independent checks and prints ONE JSON object on stdout:

    {"ok": bool, "checks": [{"id", "ok", "detail", "remediation"}, ...]}

Each check catches its own exceptions and degrades to ``ok: false`` with an
actionable, English ``remediation`` — a single failing check never crashes the
whole report. The overall ``ok`` is the AND of every check.

The six check ids, in order, are: ``runtime``, ``embedder``, ``workspace``,
``index``, ``live_server``, ``provider``.

``live_server`` and ``provider`` are stable PLACEHOLDERS the Rust/app side
overwrites with results only it can compute: ``provider`` with a vault
key-presence boolean, and ``live_server`` by probing the resident HTTP server's
(now-fast) ``/runtime`` for a ready chunk store. The data-layer checks above can
all be green while the live server is unreachable or its retrieval index is not
ready, so ``live_server`` is what makes a fully-green doctor actually mean "you
can ask Oracle and get a grounded answer".

PRIVACY (hard requirement): no ``detail`` or ``remediation`` string may contain
an absolute filesystem path, the OS username, or any secret value. Workspace /
index details report only basenames, booleans and counts — never an absolute
path. The ``provider`` check is a stable placeholder the Rust/app side overwrites
with a boolean key-presence result; it never echoes a key.

Cross-platform: pure Python, no OS-specific calls. Runs the same on Windows and
macOS, under the venv interpreter the app resolves.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

from oracle.bootstrap.warmup import check as runtime_probe
from oracle.config import (
    CHUNK_DB_PATH,
    CHUNK_MANIFEST_PATH,
    EMBED_DIMS,
    SQLITE_PATH,
)
from oracle.ingestion.embedder import embed_texts


# --- privacy helper ---------------------------------------------------------


# Path-like substrings, redacted ANYWHERE in a string (not just space-delimited
# tokens) so an embedded path such as ``key='C:\\Users\\gualt\\x'`` or
# ``path:/home/gualt/x`` cannot leak the OS username / machine layout. Conservative:
# each pattern stops at the first quote/whitespace so normal prose survives.
_PATH_PATTERNS = (
    # Windows drive paths: C:\... or C:/...
    re.compile(r"[A-Za-z]:[\\/][^\s'\"]*"),
    # Windows UNC / verbatim prefixes: \\server\... or \\?\...
    re.compile(r"\\\\[^\s'\"]*"),
    # POSIX user/home/temp paths.
    re.compile(r"/(?:Users|home|root|var/folders)/[^\s'\"]*"),
)


def _safe_detail(value: str) -> str:
    """Defense-in-depth scrub: never let an absolute path / home dir survive in a
    surfaced string. Checks build their details from basenames/booleans already;
    this is a belt-and-suspenders pass so an accidental path interpolation cannot
    leak the OS username or machine layout across the IPC boundary. Redacts
    path-like substrings wherever they appear (even when wrapped in quotes or glued
    to ``key=`` prefixes), then caps the length."""
    for pattern in _PATH_PATTERNS:
        value = pattern.sub("<path>", value)
    return value[:400]


def _check(id: str, ok: bool, detail: str = "", remediation: str = "") -> dict:
    return {
        "id": id,
        "ok": bool(ok),
        "detail": _safe_detail(detail),
        "remediation": _safe_detail(remediation) if not ok else "",
    }


# --- 1) runtime -------------------------------------------------------------


def check_runtime() -> dict:
    """venv runtime usable: LanceDB + sentence-transformers importable (cheap
    ``find_spec`` probes reused from warmup). The embedder *cache* is reported but
    not required here — the real-load check below is authoritative."""
    try:
        probe = runtime_probe()
    except Exception:
        return _check(
            "runtime",
            False,
            "Runtime probe failed.",
            "Install or repair the Oracle runtime from Oracle - Setup.",
        )
    lancedb_ok = bool(probe.get("lancedb"))
    st_ok = bool(probe.get("sentenceTransformers"))
    if lancedb_ok and st_ok:
        return _check("runtime", True, "LanceDB and sentence-transformers available.")
    missing = []
    if not lancedb_ok:
        missing.append("LanceDB")
    if not st_ok:
        missing.append("sentence-transformers")
    return _check(
        "runtime",
        False,
        f"Missing dependencies: {', '.join(missing)}.",
        "Install or repair the Oracle runtime from Oracle - Setup.",
    )


# --- 2) embedder ------------------------------------------------------------


def check_embedder(deep: bool = False) -> dict:
    """Embedder readiness via a CHEAP probe — by default this NEVER loads the
    real Qwen model (the slow, often-hanging step that made the doctor useless).

    Default mode reuses warmup's ``find_spec`` + cache-presence signals:
    ``sentenceTransformers`` (is the library importable) and ``embedderCached``
    (is the Qwen3 model materialized on disk). Both come from ``runtime_probe``
    without importing torch or constructing the model, so this returns in
    milliseconds. The "no silent MOCK at query time" guarantee does NOT depend on
    a doctor load: it is enforced at runtime by ``ORACLE_REQUIRE_REAL_EMBEDDER``
    in ``embedder.embed_texts`` (a real-model failure RAISES there, never falls
    back to hash vectors). So a cheap probe here is sufficient to be useful.

    ``deep=True`` opts into the old behaviour: a real ``embed_texts`` load plus a
    dims assertion, for explicit deep verification. The default stays cheap."""
    try:
        probe = runtime_probe()
    except Exception:
        return _check(
            "embedder",
            False,
            "Could not probe the embedder runtime.",
            "Install the Oracle runtime from Oracle - Setup.",
        )
    st_ok = bool(probe.get("sentenceTransformers"))
    cached = bool(probe.get("embedderCached"))
    if not st_ok:
        return _check(
            "embedder",
            False,
            "The sentence-transformers runtime is not installed.",
            "Install the Oracle runtime (Oracle - Setup).",
        )
    if not cached:
        return _check(
            "embedder",
            False,
            "The Qwen3 embedding model is not downloaded yet.",
            "Download the Qwen3 embedding model (Oracle - Setup - Install runtime).",
        )
    if not deep:
        return _check("embedder", True, "Qwen3 embedder installed and cached.")

    # --deep: explicit, opt-in real load + dims assertion. Slow; only run when the
    # user asks for a deep verification.
    try:
        vectors = embed_texts(["healthcheck"], require_sentence_transformer=True)
    except Exception:
        # The embedder already raises a static, path-free message; we keep our own
        # static text anyway so nothing from the exception can leak.
        return _check(
            "embedder",
            False,
            "Embedding model failed to load.",
            "Reinstall the Oracle runtime from Oracle - Setup.",
        )
    dims = len(vectors[0]) if vectors and vectors[0] else 0
    if dims != EMBED_DIMS:
        return _check(
            "embedder",
            False,
            f"Embedder returned {dims} dims, expected {EMBED_DIMS}.",
            "Reinstall the Oracle runtime from Oracle - Setup; the model is wrong.",
        )
    return _check("embedder", True, f"Embedding model loads and returns {dims} dims.")


# --- 3) workspace -----------------------------------------------------------


def check_workspace(
    root: str | None, manifest_path: Path | str = CHUNK_MANIFEST_PATH
) -> dict:
    """``root`` is set, exists, is a directory, and matches the manifest root (by
    basename) if a manifest exists. Reports only basenames / booleans — never an
    absolute path."""
    if not root or not str(root).strip():
        return _check(
            "workspace",
            False,
            "No workspace folder is selected.",
            "Open Devboule - Oracle and choose your workspace folder.",
        )
    path = Path(str(root).strip())
    if not path.exists():
        return _check(
            "workspace",
            False,
            "Selected workspace folder does not exist.",
            "Open Devboule - Oracle and choose an existing workspace folder.",
        )
    if not path.is_dir():
        return _check(
            "workspace",
            False,
            "Selected workspace path is not a folder.",
            "Open Devboule - Oracle and choose a folder, not a file.",
        )
    resolved = path.resolve()
    name = resolved.name or "workspace"
    manifest_match = _manifest_root_matches(resolved, manifest_path)
    if manifest_match is False:
        return _check(
            "workspace",
            False,
            f"Selected folder ('{name}') does not match the indexed workspace.",
            "Index this folder from Oracle - Index, or select the indexed folder.",
        )
    return _check("workspace", True, f"Workspace folder '{name}' is selected.")


def _manifest_root_matches(
    resolved_root: Path, manifest_path: Path | str
) -> bool | None:
    """Return True if the manifest's recorded root basename matches ``resolved_root``,
    False on a clear mismatch, or None when there is no manifest / no recorded root
    to compare against. NEVER surfaces an absolute path — only the boolean."""
    try:
        manifest_path = Path(manifest_path)
        if not manifest_path.is_file():
            return None
        from oracle.ingestion.chunk_index import load_manifest, manifest_roots

        manifest = load_manifest(manifest_path)
        roots = list(manifest_roots(manifest).keys())
        if not roots:
            return None
        target = resolved_root.name.lower()
        return any(Path(key).name.lower() == target for key in roots)
    except Exception:
        return None


# --- 4) index ---------------------------------------------------------------


def check_index(
    root: str | None,
    sqlite_path: Path | str = SQLITE_PATH,
    chunk_vector_path: Path | str = CHUNK_DB_PATH,
    manifest_path: Path | str = CHUNK_MANIFEST_PATH,
) -> dict:
    """LanceDB vectors + SQLite chunks exist and ``chunk_count > 0``. Mirrors the
    agents' readiness gate (``ensure_oracle_index_ready``): a non-empty workspace
    with zero indexed files or zero chunks is NOT ready. Reports expected/indexed/
    pending counts when cheaply available — never paths."""
    if not root or not str(root).strip():
        return _check(
            "index",
            False,
            "No workspace folder is selected.",
            "Open Devboule - Oracle and choose your workspace folder.",
        )
    try:
        from oracle.ingestion.chunk_index import chunk_index_status

        status = chunk_index_status(
            root=str(root),
            sqlite_path=sqlite_path,
            chunk_vector_path=chunk_vector_path,
            manifest_path=manifest_path,
        )
    except Exception:
        return _check(
            "index",
            False,
            "Could not read the index status.",
            "Index your workspace from Oracle - Index.",
        )

    expected = int(status.get("expected_files") or 0)
    indexed = int(status.get("indexed_files") or 0)
    pending = int(status.get("pending_files") or 0)
    chunks = int(status.get("sqlite_chunks") or 0)
    detail = f"expected={expected} indexed={indexed} pending={pending} chunks={chunks}"

    # Mirror ensure_oracle_index_ready EXACTLY: the agent gate raises (not-ready)
    # only when expected>0 AND (indexed==0 OR chunks==0). Everything else — including
    # expected==0 — is "ready". So "green doctor ⟺ agent ready" holds.
    if expected > 0 and (indexed == 0 or chunks == 0):
        return _check(
            "index",
            False,
            "The workspace is not indexed yet. " + detail,
            "Index your workspace from Oracle - Index.",
        )
    if expected == 0:
        # A non-empty workspace whose every file was filtered out, or an empty dir.
        # The agent gate treats this as ready, so the doctor does too — but we keep
        # an informative, NON-failing note pointing at the filter. (Remediation is
        # blanked for ok checks by _check, so the note lives in detail.)
        return _check(
            "index",
            True,
            "No indexable files (all excluded by the secret / .oracleignore "
            "filter, or empty workspace). " + detail,
        )
    return _check("index", True, detail)


# --- 5) live_server (placeholder, overwritten by the app/Rust side) ---------


def live_server_placeholder() -> dict:
    """Stable placeholder for the LIVE resident-server check.

    Only the Rust/app side knows the session port + auth token of the resident
    Oracle server, so it overwrites this by id with the real result: the server
    is reachable AND its ``/runtime`` reports the CHUNK store ready (records>0).
    Emitted ``ok: true`` so a STANDALONE doctor run (no app, e.g. the CLI/tests)
    does not flip the report red over a server the doctor was never wired to
    reach; the app ALWAYS overwrites it with the authoritative live result.
    NEVER echoes a port, token, or path."""
    return {
        "id": "live_server",
        "ok": True,
        "detail": "checked by app",
        "remediation": "",
    }


# --- 6) provider (placeholder, overwritten by the app/Rust side) ------------


def provider_placeholder() -> dict:
    """Python cannot read the OS vault, so emit a stable placeholder the Rust side
    find/replaces by id with a boolean key-presence result. NEVER echoes a key."""
    return {
        "id": "provider",
        "ok": True,
        "detail": "checked by app",
        "remediation": "",
    }


# --- report -----------------------------------------------------------------


def build_report(
    root: str | None,
    sqlite_path: Path | str = SQLITE_PATH,
    chunk_vector_path: Path | str = CHUNK_DB_PATH,
    manifest_path: Path | str = CHUNK_MANIFEST_PATH,
    deep: bool = False,
) -> dict:
    checks = [
        check_runtime(),
        check_embedder(deep=deep),
        check_workspace(root, manifest_path=manifest_path),
        check_index(
            root,
            sqlite_path=sqlite_path,
            chunk_vector_path=chunk_vector_path,
            manifest_path=manifest_path,
        ),
        live_server_placeholder(),
        provider_placeholder(),
    ]
    return {"ok": all(check["ok"] for check in checks), "checks": checks}


def main(argv: list[str] | None = None) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description="Oracle health doctor")
    parser.add_argument("--root", default=None, help="indexed workspace root")
    parser.add_argument(
        "--deep",
        action="store_true",
        help="deep embedder verification: actually load the model and assert "
        "dims (slow). Default mode stays cheap and never loads the model.",
    )
    args = parser.parse_args(argv)

    report = build_report(args.root, deep=args.deep)
    print(json.dumps(report, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
