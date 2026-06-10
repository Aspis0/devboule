import json
from pathlib import Path

from oracle.config import CHUNK_DB_PATH, CHUNK_MANIFEST_PATH, LANCE_DB_PATH, LLM_MODEL, SQLITE_PATH
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


def runtime_status(vector_path: Path | str = LANCE_DB_PATH, llm_model: str = LLM_MODEL) -> dict:
    """Report retrieval-runtime status (vector store + chunk store).

    Oracle answers are API-only now; the local Ollama chat path has been
    removed. A vestigial, always-disabled ``ollama`` object and an empty
    ``setup_commands`` list are still returned so the Tauri host's
    OracleRuntime payload shape stays backward compatible. The local embedder
    and the LanceDB index are unaffected and remain mandatory for retrieval.
    """
    vector_store = LanceStore(vector_path)
    sqlite = SQLiteStore(SQLITE_PATH)
    chunk_store = LanceStore(CHUNK_DB_PATH)

    # Count each store exactly once. LanceStore.count() now uses the native
    # `count_rows()` (a metadata read, ~ms) so this stays cheap enough for a
    # readiness probe. SQLite chunk counts are plain COUNT(*) queries.
    vector_records = vector_store.count()
    chunk_vector_records = chunk_store.count()
    chunk_files = sqlite.chunk_file_count()
    chunk_records = sqlite.chunk_count()

    # AUTHORITATIVE retrieval readiness is the CHUNK store: dense retrieval and
    # `/ask` search `chunks.lancedb` + the SQLite chunk table, NOT the legacy
    # node-level `vectors.lancedb`. Graph/node vectors are no longer produced, so
    # `vectors.lancedb` is typically empty and MUST NOT make the runtime look
    # "not ready". A store is ready when it holds vectors AND the matching SQLite
    # rows exist (so a half-written index does not falsely read ready).
    chunk_ready = chunk_vector_records > 0 and chunk_records > 0
    return {
        "vector_store": {
            "backend": vector_store.backend,
            "path": str(vector_store.path),
            "records": vector_records,
            "ready": vector_records > 0,
        },
        "chunk_store": {
            "backend": chunk_store.backend,
            "path": str(chunk_store.path),
            "manifest_path": str(CHUNK_MANIFEST_PATH),
            **chunk_manifest_status(CHUNK_MANIFEST_PATH),
            "files": chunk_files,
            "records": chunk_records,
            "vector_records": chunk_vector_records,
            "ready": chunk_ready,
        },
        # Top-level retrieval readiness for the host/UI: the chunk store is the
        # real index, so this mirrors `chunk_store.ready`. Surfaced at the top
        # level (additively) so the Rust/UI readiness gate does not have to know
        # which nested store is authoritative.
        "ready": chunk_ready,
        "ollama": disabled_ollama(llm_model),
        "setup_commands": setup_commands(),
    }


def chunk_manifest_status(path: Path | str = CHUNK_MANIFEST_PATH) -> dict:
    path = Path(path)
    if not path.exists():
        return {"manifest_root": None, "manifest_files": 0}
    try:
        payload = json.loads(path.read_text(encoding="utf-8") or "{}")
        return {
            "manifest_root": payload.get("root"),
            "manifest_files": len(payload.get("files", {})),
        }
    except Exception:
        return {"manifest_root": None, "manifest_files": 0}


def disabled_ollama(llm_model: str = LLM_MODEL) -> dict:
    """Vestigial, always-disabled Ollama status (local chat path removed)."""
    return {
        "cli": None,
        "server": "removed",
        "model": llm_model,
        "model_available": False,
        "models": [],
        "message": "Local Ollama chat path removed; Oracle answers are API-only.",
    }


def model_names(payload: object) -> set[str]:
    """Extract model names from a dict/object payload.

    Retained for backward compatibility with existing tests and callers; the
    local Ollama chat path is gone, but this pure helper is harmless.
    """
    if isinstance(payload, dict):
        raw_models = payload.get("models", [])
    else:
        raw_models = getattr(payload, "models", [])

    names: set[str] = set()
    for item in raw_models:
        if isinstance(item, dict):
            value = item.get("name") or item.get("model")
        else:
            value = getattr(item, "name", None) or getattr(item, "model", None)
        if value:
            names.add(str(value))
    return names


def setup_commands(llm_model: str = LLM_MODEL) -> list[str]:
    """No local-LLM setup is required anymore (answers are API-only)."""
    return []
