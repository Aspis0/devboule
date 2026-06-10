"""Oracle runtime warmup / readiness probe.

Two modes, both print a single JSON object on stdout:

* ``--check`` — cheap readiness probe. Uses ``importlib`` to see whether LanceDB
  and sentence-transformers are installed (without importing torch) and whether
  the Qwen3 embedding model is already cached. Never raises on missing deps;
  reports booleans instead. Used by the Rust status command.

* default (warm) — installs nothing, but downloads + loads the embedding model
  and verifies LanceDB actually works, so the first real query is fast and works
  offline afterwards. Raises (non-zero exit) on failure so the bootstrap can
  surface the error.

Cross-platform: pure Python, no OS-specific calls. Runs the same on Windows and
macOS.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys

from oracle.config import CHUNK_DB_PATH, EMBED_MODEL


def _module_installed(name: str) -> bool:
    try:
        return importlib.util.find_spec(name) is not None
    except Exception:
        return False


def _embedder_cached() -> bool:
    if not _module_installed("huggingface_hub"):
        return False
    try:
        from huggingface_hub import try_to_load_from_cache

        cached = try_to_load_from_cache(EMBED_MODEL, "config.json")
        return isinstance(cached, str) and os.path.exists(cached)
    except Exception:
        return False


def check() -> dict:
    return {
        "lancedb": _module_installed("lancedb"),
        "sentenceTransformers": _module_installed("sentence_transformers"),
        "embedderCached": _embedder_cached(),
        "embedModel": EMBED_MODEL,
    }


def warm() -> dict:
    os.environ["ORACLE_ALLOW_HF_DOWNLOAD"] = "1"

    # Verify LanceDB is importable and can open a connection at the chunk store.
    import lancedb

    CHUNK_DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    lancedb.connect(str(CHUNK_DB_PATH))

    # Download + load the embedder and run one real encode so the model is fully
    # materialized and cached locally.
    from oracle.ingestion.embedder import embed_texts

    vectors = embed_texts(
        ["oracle runtime warmup"],
        use_sentence_transformer=True,
        require_sentence_transformer=True,
    )
    dims = len(vectors[0]) if vectors and vectors[0] else 0
    if dims <= 0:
        raise RuntimeError("Embedder returned an empty vector during warmup.")
    return {
        "ok": True,
        "lancedb": True,
        "embedderCached": True,
        "embedDims": dims,
        "embedModel": EMBED_MODEL,
    }


def main(argv: list[str] | None = None) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description="Oracle runtime warmup / readiness probe")
    parser.add_argument("--check", action="store_true", help="cheap readiness probe only")
    args = parser.parse_args(argv)

    payload = check() if args.check else warm()
    # Sentinel prefix so the Rust probe never mistakes import-time chatter for
    # the result line.
    print(f"ORACLE_RUNTIME_CHECK {json.dumps(payload, ensure_ascii=False)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
