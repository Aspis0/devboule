import json
import sqlite3
from pathlib import Path
from contextlib import contextmanager


ARRAY_FIELDS = {"espone_api", "dipende_da", "simile_a", "tecnologie"}


class SQLiteStore:
    def __init__(self, path: Path | str):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._init_schema()

    # P3: how long a reader/writer waits for a competing lock before raising
    # "database is locked", in milliseconds. The index write transaction can hold
    # the database briefly; without a real busy timeout the reader endpoints
    # (/health, /runtime, /ask) raised immediately. 5s comfortably covers a chunk
    # write commit while still bounding a genuine deadlock.
    _BUSY_TIMEOUT_MS = 5000

    @contextmanager
    def _connect(self):
        conn = sqlite3.connect(self.path, timeout=self._BUSY_TIMEOUT_MS / 1000)
        try:
            # P3: WAL lets readers proceed CONCURRENTLY with a writer (in the
            # default rollback-journal mode a write transaction blocks all readers,
            # which is what stalled /runtime, /health and /ask behind an index
            # write — surfacing as "always indexing" / "Checking vector runtime"
            # hangs). WAL is safe for this access pattern: a single resident server
            # process owns the database, all access is local (no networked FS), and
            # `journal_mode=WAL` is a persistent, idempotent per-database setting.
            # `busy_timeout` is per-connection, so it is (re)applied on every
            # connect; it makes a reader/writer WAIT for a transient lock instead of
            # raising "database is locked" immediately.
            conn.execute(f"PRAGMA busy_timeout={self._BUSY_TIMEOUT_MS}")
            conn.execute("PRAGMA journal_mode=WAL")
            yield conn
            conn.commit()
        finally:
            conn.close()

    def _init_schema(self) -> None:
        with self._connect() as conn:
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS node_cards (
                  id TEXT PRIMARY KEY,
                  label TEXT NOT NULL,
                  area TEXT NOT NULL,
                  cluster_semantic TEXT NOT NULL,
                  funzione_primaria TEXT NOT NULL,
                  espone_api TEXT NOT NULL,
                  dipende_da TEXT NOT NULL,
                  simile_a TEXT NOT NULL,
                  tecnologie TEXT NOT NULL,
                  file_sorgente TEXT NOT NULL,
                  ultima_modifica TEXT NOT NULL,
                  source TEXT NOT NULL,
                  embedding_dims INTEGER NOT NULL
                )
                """
            )
            conn.execute("CREATE INDEX IF NOT EXISTS idx_node_area ON node_cards(area)")
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_node_cluster ON node_cards(cluster_semantic)"
            )
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_node_label ON node_cards(label)"
            )
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS file_chunks (
                  id TEXT PRIMARY KEY,
                  file_id TEXT NOT NULL,
                  chunk_index INTEGER NOT NULL,
                  start_char INTEGER NOT NULL,
                  end_char INTEGER NOT NULL,
                  text TEXT NOT NULL,
                  file_sorgente TEXT NOT NULL,
                  ultima_modifica TEXT NOT NULL,
                  embedding_dims INTEGER NOT NULL,
                  kind TEXT NOT NULL DEFAULT '',
                  symbol_name TEXT NOT NULL DEFAULT '',
                  signature TEXT NOT NULL DEFAULT '',
                  line_start INTEGER NOT NULL DEFAULT 0,
                  line_end INTEGER NOT NULL DEFAULT 0,
                  language TEXT NOT NULL DEFAULT '',
                  symbols_used TEXT NOT NULL DEFAULT ''
                )
                """
            )
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_chunk_file ON file_chunks(file_id)"
            )

    def upsert_many(self, cards: list[dict]) -> None:
        with self._connect() as conn:
            conn.executemany(
                """
                INSERT INTO node_cards (
                  id, label, area, cluster_semantic, funzione_primaria, espone_api,
                  dipende_da, simile_a, tecnologie, file_sorgente, ultima_modifica,
                  source, embedding_dims
                ) VALUES (
                  :id, :label, :area, :cluster_semantic, :funzione_primaria, :espone_api,
                  :dipende_da, :simile_a, :tecnologie, :file_sorgente, :ultima_modifica,
                  :source, :embedding_dims
                )
                ON CONFLICT(id) DO UPDATE SET
                  label=excluded.label,
                  area=excluded.area,
                  cluster_semantic=excluded.cluster_semantic,
                  funzione_primaria=excluded.funzione_primaria,
                  espone_api=excluded.espone_api,
                  dipende_da=excluded.dipende_da,
                  simile_a=excluded.simile_a,
                  tecnologie=excluded.tecnologie,
                  file_sorgente=excluded.file_sorgente,
                  ultima_modifica=excluded.ultima_modifica,
                  source=excluded.source,
                  embedding_dims=excluded.embedding_dims
                """,
                [self._serialize(card) for card in cards],
            )

    def replace_all(self, cards: list[dict]) -> None:
        with self._connect() as conn:
            conn.execute("DELETE FROM node_cards")
        self.upsert_many(cards)

    def delete_nodes(self, node_ids: list[str]) -> None:
        if not node_ids:
            return
        with self._connect() as conn:
            conn.executemany(
                "DELETE FROM node_cards WHERE id = ?",
                [(node_id,) for node_id in node_ids],
            )

    def get_node(self, node_id: str) -> dict | None:
        with self._connect() as conn:
            row = conn.execute(
                "SELECT * FROM node_cards WHERE id = ?",
                (node_id,),
            ).fetchone()
            columns = [col[1] for col in conn.execute("PRAGMA table_info(node_cards)")]
        return self._deserialize(columns, row) if row else None

    def all_nodes(self) -> list[dict]:
        with self._connect() as conn:
            rows = conn.execute("SELECT * FROM node_cards ORDER BY id").fetchall()
            columns = [col[1] for col in conn.execute("PRAGMA table_info(node_cards)")]
        return [self._deserialize(columns, row) for row in rows]

    def by_cluster(self, cluster: str) -> list[dict]:
        return [
            node
            for node in self.all_nodes()
            if node["cluster_semantic"].lower() == cluster.lower()
        ]

    def by_area(self, area: str) -> list[dict]:
        return [
            node for node in self.all_nodes() if node["area"].lower() == area.lower()
        ]

    def count(self) -> int:
        with self._connect() as conn:
            return int(conn.execute("SELECT COUNT(*) FROM node_cards").fetchone()[0])

    def replace_chunks_for_files(self, file_ids: list[str], chunks: list[dict]) -> None:
        with self._connect() as conn:
            conn.executemany(
                "DELETE FROM file_chunks WHERE file_id = ?",
                [(file_id,) for file_id in file_ids],
            )
            conn.executemany(
                """
                INSERT INTO file_chunks (
                  id, file_id, chunk_index, start_char, end_char, text,
                  file_sorgente, ultima_modifica, embedding_dims,
                  kind, symbol_name, signature, line_start, line_end, language, symbols_used
                ) VALUES (
                  :id, :file_id, :chunk_index, :start_char, :end_char, :text,
                  :file_sorgente, :ultima_modifica, :embedding_dims,
                  :kind, :symbol_name, :signature, :line_start, :line_end, :language, :symbols_used
                )
                ON CONFLICT(id) DO UPDATE SET
                  file_id=excluded.file_id,
                  chunk_index=excluded.chunk_index,
                  start_char=excluded.start_char,
                  end_char=excluded.end_char,
                  text=excluded.text,
                  file_sorgente=excluded.file_sorgente,
                  ultima_modifica=excluded.ultima_modifica,
                  embedding_dims=excluded.embedding_dims,
                  kind=excluded.kind,
                  symbol_name=excluded.symbol_name,
                  signature=excluded.signature,
                  line_start=excluded.line_start,
                  line_end=excluded.line_end,
                  language=excluded.language,
                  symbols_used=excluded.symbols_used
                """,
                chunks,
            )

    def replace_all_chunks(self, chunks: list[dict]) -> None:
        with self._connect() as conn:
            conn.execute("DELETE FROM file_chunks")
        if chunks:
            self.replace_chunks_for_files(
                sorted({chunk["file_id"] for chunk in chunks}), chunks
            )

    def chunk_ids_for_files(self, file_ids: list[str]) -> list[str]:
        if not file_ids:
            return []
        placeholders = ",".join("?" for _ in file_ids)
        with self._connect() as conn:
            rows = conn.execute(
                f"SELECT id FROM file_chunks WHERE file_id IN ({placeholders})",
                file_ids,
            ).fetchall()
        return [row[0] for row in rows]

    def chunks_for_file(self, file_id: str) -> list[dict]:
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM file_chunks WHERE file_id = ? ORDER BY chunk_index",
                (file_id,),
            ).fetchall()
            columns = [col[1] for col in conn.execute("PRAGMA table_info(file_chunks)")]
        return [self._deserialize_chunk(dict(zip(columns, row))) for row in rows]

    def get_chunk(self, chunk_id: str) -> dict | None:
        with self._connect() as conn:
            row = conn.execute(
                "SELECT * FROM file_chunks WHERE id = ?", (chunk_id,)
            ).fetchone()
            columns = [col[1] for col in conn.execute("PRAGMA table_info(file_chunks)")]
        return self._deserialize_chunk(dict(zip(columns, row))) if row else None

    def all_chunks(self) -> list[dict]:
        with self._connect() as conn:
            rows = conn.execute("SELECT * FROM file_chunks ORDER BY id").fetchall()
            columns = [col[1] for col in conn.execute("PRAGMA table_info(file_chunks)")]
        return [self._deserialize_chunk(dict(zip(columns, row))) for row in rows]

    @staticmethod
    def _deserialize_chunk(chunk: dict) -> dict:
        """Convert JSON-stored fields back to Python objects."""
        syms = chunk.get("symbols_used")
        if isinstance(syms, str) and syms:
            try:
                chunk["symbols_used"] = json.loads(syms)
            except (json.JSONDecodeError, TypeError):
                chunk["symbols_used"] = []
        elif not isinstance(syms, list):
            chunk["symbols_used"] = []
        return chunk

    def chunk_count(self) -> int:
        with self._connect() as conn:
            return int(conn.execute("SELECT COUNT(*) FROM file_chunks").fetchone()[0])

    def chunk_file_count(self) -> int:
        with self._connect() as conn:
            return int(
                conn.execute(
                    "SELECT COUNT(DISTINCT file_id) FROM file_chunks"
                ).fetchone()[0]
            )

    def _serialize(self, card: dict) -> dict:
        out = dict(card)
        for field in ARRAY_FIELDS:
            out[field] = json.dumps(out.get(field, []), ensure_ascii=False)
        return out

    def _deserialize(self, columns: list[str], row: tuple) -> dict:
        out = dict(zip(columns, row))
        for field in ARRAY_FIELDS:
            out[field] = json.loads(out[field] or "[]")
        return out
