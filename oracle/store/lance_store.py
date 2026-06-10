import hashlib
import json
import logging
import math
import os
import re
import threading
from pathlib import Path

from oracle.ingestion.retrieval_text import query_embedding_text


logger = logging.getLogger(__name__)

TOKEN_RE = re.compile(r"[A-Za-z0-9_/-]+")

# Process-wide cache of opened LanceDB connections, keyed by the resolved DB
# directory. A fresh `make_engine()` (hence a fresh `LanceStore`) is built per
# HTTP request, so an instance-level handle would be reopened every call — the
# readiness path would then pay a full `lancedb.connect` + table open on every
# `/health`/`/runtime` probe. Caching the connection here keeps the hot
# readiness path (which only needs a row COUNT, not the rows) cheap and stable
# across requests. The cache stores the lightweight `lancedb` Connection object,
# NOT row data, so it never goes stale on writes: tables are re-opened from the
# live connection and `count_rows()` always reflects the current on-disk state.
_CONNECTION_CACHE: dict[str, object] = {}
_CONNECTION_CACHE_LOCK = threading.Lock()


def embed_text(text: str, dims: int = 1024) -> list[float]:
    vector = [0.0] * dims
    for token in TOKEN_RE.findall(text.lower()):
        digest = hashlib.blake2b(token.encode("utf-8"), digest_size=8).digest()
        index = int.from_bytes(digest[:4], "little") % dims
        sign = 1.0 if digest[4] % 2 == 0 else -1.0
        vector[index] += sign
    norm = math.sqrt(sum(value * value for value in vector)) or 1.0
    return [value / norm for value in vector]


def cosine(a: list[float], b: list[float]) -> float:
    return sum(x * y for x, y in zip(a, b))


def embed_query_text(text: str, dims: int = 1024) -> list[float]:
    # Lazy import avoids the embedder<->lance_store circular import at module load.
    from oracle.ingestion.embedder import _sentence_model, require_real_embedder

    require_real = require_real_embedder()
    # The production hard-switch DOMINATES the ORACLE_QUERY_EMBEDDER=hash debug
    # knob: when require_real is set, a hash query vector (which retrieves
    # garbage) can never be produced, so the debug knob is ignored.
    if not require_real and os.getenv("ORACLE_QUERY_EMBEDDER", "").lower() == "hash":
        return embed_text(text, dims=dims)
    try:
        model = _sentence_model()
        embedding = model.encode([text], show_progress_bar=False)[0]
        return [float(value) for value in embedding]
    except Exception as exc:
        if require_real:
            # Full detail (paths/usernames from torch/HF) stays in logs only; the
            # surfaced message is static so it never leaks to the Python HTTP/MCP
            # response bodies that the Rust sanitizer does not cover.
            logger.error("Qwen query embedding failed: %s", exc)
            raise RuntimeError(
                "Qwen embedding model is unavailable. "
                "Run Oracle doctor / check the runtime install."
            ) from exc
        # Test-only hash mock: reachable ONLY when ORACLE_REQUIRE_REAL_EMBEDDER is
        # unset. Never reached in production where the env is set.
        return embed_text(text, dims=dims)


class LanceStore:
    """Embedded vector adapter.

    Uses LanceDB for `.lancedb` paths. JSON paths remain supported as a small
    deterministic fallback for tests and recovery on machines without LanceDB.
    """

    def __init__(self, path: Path | str):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.backend = "json" if self.path.suffix == ".json" else "lancedb"

    def upsert(self, records: list[dict]) -> None:
        existing = {record["id"]: record for record in self._read()}
        for record in records:
            existing[record["id"]] = {
                "id": record["id"],
                "label": record.get("label", record["id"]),
                "area": record.get("area", "unknown"),
                "cluster_semantic": record.get("cluster_semantic", "unknown"),
                "vector": record["vector"],
            }
        self._write_all(list(existing.values()))

    def replace_ids(self, delete_ids: list[str], records: list[dict]) -> None:
        records = [self._vector_record(record) for record in records]
        delete_ids = sorted(set(delete_ids) | {record["id"] for record in records})
        if self.backend == "json":
            existing = {
                record["id"]: record
                for record in self._read()
                if record["id"] not in set(delete_ids)
            }
            for record in records:
                existing[record["id"]] = record
            self._write_all(list(existing.values()))
            return

        self.path.mkdir(parents=True, exist_ok=True)
        db = self._connect()
        table = self._open_lance_table()
        if table is not None and delete_ids:
            try:
                for batch in _chunks(delete_ids, 200):
                    quoted = ",".join(_quote_sql_string(value) for value in batch)
                    table.delete(f"id IN ({quoted})")
            except Exception:
                existing = [
                    record
                    for record in self._read()
                    if record["id"] not in set(delete_ids)
                ]
                self._write_all(existing + records)
                return

        if not records:
            return
        table = self._open_lance_table()
        if table is None:
            db.create_table("nodes", data=records, mode="overwrite")
        else:
            table.add(records)

    def replace_all(self, records: list[dict]) -> None:
        self._write_all([self._vector_record(record) for record in records])

    def search(self, query: str, limit: int = 5) -> list[dict]:
        records = self._read()
        if not records:
            return []
        dims = len(records[0].get("vector", []))
        query_vector = embed_query_text(query_embedding_text(query), dims=dims)
        rows = []
        for record in records:
            rows.append({**record, "score": cosine(query_vector, record.get("vector", []))})
        rows.sort(key=lambda item: (-item["score"], item["id"]))
        return rows[: max(1, limit)]

    def similar(self, node_id: str, limit: int = 5) -> list[dict]:
        records = self._read()
        source = next((record for record in records if record["id"] == node_id), None)
        if not source:
            return []
        rows = []
        for record in records:
            if record["id"] == node_id:
                continue
            rows.append({**record, "score": cosine(source["vector"], record["vector"])})
        rows.sort(key=lambda item: (-item["score"], item["id"]))
        return rows[: max(1, limit)]

    def count(self) -> int:
        # Cheap, exact row count. For LanceDB this uses the native
        # `count_rows()` (a metadata read, ~milliseconds) instead of materializing
        # every row (`_read()` deserialized all vectors — ~2s for a few thousand
        # rows, which is what made the readiness probes flake on their ~5s
        # timeout). The JSON fallback still counts the small in-file list.
        if self.backend == "json":
            return len(self._read())
        table = self._open_lance_table()
        if table is None:
            return 0
        try:
            return int(table.count_rows())
        except Exception:
            # Defensive: an older LanceDB without `count_rows()` (or a transient
            # read error) degrades to the full scan rather than crashing the
            # readiness probe.
            return len(self._read())

    def ids(self) -> list[str]:
        return [record["id"] for record in self._read()]

    def _read(self) -> list[dict]:
        if self.backend == "json":
            if not self.path.exists():
                return []
            return json.loads(self.path.read_text(encoding="utf-8") or "[]")

        table = self._open_lance_table()
        if table is None:
            return []
        return [self._normalize_lance_row(row) for row in table.to_arrow().to_pylist()]

    def _write_all(self, records: list[dict]) -> None:
        records = [self._vector_record(record) for record in records]
        if self.backend == "json":
            self.path.write_text(json.dumps(records, ensure_ascii=False), encoding="utf-8")
            return

        self.path.mkdir(parents=True, exist_ok=True)
        db = self._connect()
        if records:
            db.create_table("nodes", data=records, mode="overwrite")
        elif "nodes" in self._table_names(db):
            db.drop_table("nodes")

    def _connect(self):
        """Return a process-cached LanceDB connection for this DB directory.

        Opening a connection is the expensive part of a readiness probe; caching
        it per path keeps repeated `/health`/`/runtime` calls cheap. The cached
        object is only the connection (no row data), so reads through it always
        reflect the current on-disk tables. A connect failure is not cached.
        """
        import lancedb

        key = str(self.path)
        cached = _CONNECTION_CACHE.get(key)
        if cached is not None:
            return cached
        with _CONNECTION_CACHE_LOCK:
            cached = _CONNECTION_CACHE.get(key)
            if cached is not None:
                return cached
            db = lancedb.connect(self.path)
            _CONNECTION_CACHE[key] = db
            return db

    def _open_lance_table(self):
        try:
            if not self.path.exists():
                return None
            db = self._connect()
            if "nodes" not in self._table_names(db):
                return None
            return db.open_table("nodes")
        except Exception:
            return None

    def _table_names(self, db) -> list[str]:
        names = db.list_tables()
        if isinstance(names, list):
            return names
        return list(getattr(names, "tables", []))

    def _vector_record(self, record: dict) -> dict:
        return {
            "id": record["id"],
            "label": record.get("label", record["id"]),
            "area": record.get("area", "unknown"),
            "cluster_semantic": record.get("cluster_semantic", "unknown"),
            "vector": [float(value) for value in record["vector"]],
        }

    def _normalize_lance_row(self, row: dict) -> dict:
        return {
            "id": row["id"],
            "label": row.get("label", row["id"]),
            "area": row.get("area", "unknown"),
            "cluster_semantic": row.get("cluster_semantic", "unknown"),
            "vector": [float(value) for value in row.get("vector", [])],
        }


def _chunks(items: list[str], size: int):
    for index in range(0, len(items), size):
        yield items[index : index + size]


def _quote_sql_string(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"
