import sqlite3
from pathlib import Path
from contextlib import contextmanager


class CkgStore:
    """SQLite store for the code-knowledge-graph (symbol nodes + typed edges). Sibling of the
    chunk/vector stores (see SQLiteStore) — same WAL + busy-timeout access pattern so a single
    resident server can read while a re-index writes. Populated from the Rust `ckg` CLI bridge."""

    _BUSY_TIMEOUT_MS = 5000

    def __init__(self, path: Path | str):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._init_schema()

    @contextmanager
    def _connect(self):
        conn = sqlite3.connect(self.path, timeout=self._BUSY_TIMEOUT_MS / 1000)
        try:
            conn.execute(f"PRAGMA busy_timeout={self._BUSY_TIMEOUT_MS}")
            conn.execute("PRAGMA journal_mode=WAL")
            yield conn
            conn.commit()
        finally:
            conn.close()

    def _init_schema(self) -> None:
        with self._connect() as conn:
            conn.executescript(
                """
                CREATE TABLE IF NOT EXISTS ckg_nodes (
                    id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    name TEXT,
                    file TEXT NOT NULL,
                    start_line INTEGER,
                    end_line INTEGER,
                    lang TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_ckg_nodes_file ON ckg_nodes(file);
                CREATE INDEX IF NOT EXISTS idx_ckg_nodes_name ON ckg_nodes(name);

                CREATE TABLE IF NOT EXISTS ckg_edges (
                    src TEXT NOT NULL,
                    dst TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    src_file TEXT NOT NULL,
                    PRIMARY KEY (src, dst, kind)
                );
                CREATE INDEX IF NOT EXISTS idx_ckg_edges_dst ON ckg_edges(dst, kind);
                CREATE INDEX IF NOT EXISTS idx_ckg_edges_src ON ckg_edges(src, kind);
                CREATE INDEX IF NOT EXISTS idx_ckg_edges_srcfile ON ckg_edges(src_file);
                """
            )

    @staticmethod
    def _insert(conn, nodes: list[dict], edges: list[dict]) -> None:
        if nodes:
            conn.executemany(
                "INSERT OR REPLACE INTO ckg_nodes (id, kind, name, file, start_line, end_line, lang) "
                "VALUES (:id, :kind, :name, :file, :startLine, :endLine, :lang)",
                nodes,
            )
        if edges:
            conn.executemany(
                "INSERT OR REPLACE INTO ckg_edges (src, dst, kind, src_file) "
                "VALUES (:src, :dst, :kind, :srcFile)",
                edges,
            )

    def replace_for_files(self, files: list[str], nodes: list[dict], edges: list[dict]) -> None:
        """Incremental delta primitive: in ONE transaction, drop every node/edge belonging to
        `files` (nodes by `file`, edges by `src_file`) then insert the supplied nodes/edges."""
        with self._connect() as conn:
            if files:
                placeholders = ",".join("?" * len(files))
                conn.execute(f"DELETE FROM ckg_nodes WHERE file IN ({placeholders})", files)
                conn.execute(f"DELETE FROM ckg_edges WHERE src_file IN ({placeholders})", files)
            self._insert(conn, nodes, edges)

    def replace_all(self, nodes: list[dict], edges: list[dict]) -> None:
        """Full rebuild: wipe both tables then bulk-insert."""
        with self._connect() as conn:
            conn.execute("DELETE FROM ckg_nodes")
            conn.execute("DELETE FROM ckg_edges")
            self._insert(conn, nodes, edges)

    def find_imports(self, file: str) -> list[dict]:
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT src, dst, kind FROM ckg_edges WHERE src_file = ? AND kind = 'IMPORT'",
                (file,),
            ).fetchall()
            return [{"src": r[0], "dst": r[1], "kind": r[2]} for r in rows]

    def get_neighborhood(self, node_id: str, k: int, kind: str | None = None) -> list[dict]:
        """k-hop outward neighborhood via a recursive CTE (optionally filtered to one edge kind)."""
        with self._connect() as conn:
            rows = conn.execute(
                """
                WITH RECURSIVE nbr(id, depth) AS (
                    SELECT :start, 0
                    UNION
                    SELECT e.dst, n.depth + 1 FROM ckg_edges e JOIN nbr n ON e.src = n.id
                    WHERE n.depth < :k AND (:kind IS NULL OR e.kind = :kind)
                )
                SELECT DISTINCT id, depth FROM nbr WHERE id != :start
                """,
                {"start": node_id, "k": k, "kind": kind},
            ).fetchall()
            return [{"id": r[0], "depth": r[1]} for r in rows]

    def find_callers(self, name: str) -> list[dict]:
        """Nodes with a CALL edge TO any node named `name`. Empty until CALL edges land (B3)."""
        with self._connect() as conn:
            rows = conn.execute(
                """
                SELECT e.src, e.dst
                FROM ckg_edges e
                JOIN ckg_nodes n ON e.dst = n.id
                WHERE n.name = ? AND e.kind = 'CALL'
                """,
                (name,),
            ).fetchall()
            return [{"src": r[0], "dst": r[1]} for r in rows]


def test_ckg_store_roundtrip(tmp_path: Path) -> None:
    store = CkgStore(tmp_path / "ckg.sqlite")
    nodes = [
        {"id": "f.py", "kind": "FILE", "name": None, "file": "f.py",
         "startLine": 1, "endLine": 10, "lang": "Python"},
        {"id": "f.py#2-3-0", "kind": "function_definition", "name": "foo", "file": "f.py",
         "startLine": 2, "endLine": 3, "lang": "Python"},
    ]
    edges = [{"src": "f.py", "dst": "f.py#2-3-0", "kind": "CONTAIN", "srcFile": "f.py"}]

    store.replace_all(nodes, edges)
    nbr = store.get_neighborhood("f.py", 1)
    assert len(nbr) == 1
    assert nbr[0]["id"] == "f.py#2-3-0"
    assert nbr[0]["depth"] == 1

    store.replace_for_files(["f.py"], [], [])
    assert store.get_neighborhood("f.py", 1) == []
