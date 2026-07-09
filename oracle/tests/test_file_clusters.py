"""P6.1 — File-level embedding cluster tests.

Tests: mean-pool math, <8 files skip, KMeans deterministic path,
replace-all transaction, routes shape, unknown cluster fail-open, epoch stamping.
"""

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

# Force hash-based embedder to avoid loading the sentence transformer.
os.environ.setdefault("ORACLE_REQUIRE_REAL_EMBEDDER", "0")


def _make_distinct_chunk_vector(file_idx: int, chunk_idx: int, dims: int = 128) -> list[float]:
    """Return a distinct-but-deterministic unit vector per (file, chunk)."""
    import math
    seed_val = (file_idx * 1000 + chunk_idx + 1) * 0.37
    v = [math.sin(seed_val + i * 0.17) for i in range(dims)]
    norm = math.sqrt(sum(x * x for x in v))
    return [x / norm for x in v]


def _seed_sqlite_and_vectors(sqlite_path: Path, chunk_vector_path: Path,
                              files: list[str], chunks_per_file: int = 3):
    """Seed sqlite file_chunks + chunk lancedb for a list of file ids.

    Each file gets `chunks_per_file` chunks with distinct vectors.
    Returns the SQLiteStore instance (vectors store is written to disk).
    """
    from oracle.store.sqlite_store import SQLiteStore
    from oracle.store.lance_store import LanceStore

    sqlite = SQLiteStore(sqlite_path)

    chunk_recs = []
    vector_recs = []
    for fi, fid in enumerate(files):
        for ci in range(chunks_per_file):
            cid = f"{fid}__chunk{ci}"
            chunk_recs.append({
                "id": cid,
                "file_id": fid,
                "chunk_index": ci,
                "start_char": ci * 100,
                "end_char": (ci + 1) * 100 - 1,
                "text": f"chunk {ci} of {fid}",
                "file_sorgente": fid,
                "ultima_modifica": "2025-01-01T00:00:00Z",
                "embedding_dims": 0,
                "kind": "",
                "symbol_name": "",
                "signature": "",
                "line_start": ci * 10,
                "line_end": (ci + 1) * 10 - 1,
                "language": "",
                "symbols_used": "[]",
            })
            vector_recs.append({
                "id": cid,
                "label": cid,
                "area": "unknown",
                "cluster_semantic": "unknown",
                "vector": _make_distinct_chunk_vector(fi, ci),
            })

    sqlite.replace_chunks_for_files(
        list(set(c["file_id"] for c in chunk_recs)), chunk_recs
    )

    vectors = LanceStore(chunk_vector_path)
    vectors.replace_all(vector_recs)

    return sqlite


class MeanPoolMathTest(unittest.TestCase):
    """Verify the mean-pool logic used by _refresh_clusters."""

    def test_mean_pool_averages_vectors(self):
        import numpy as np

        v0 = [1.0, 0.0, 0.0]
        v1 = [0.0, 1.0, 0.0]
        v2 = [0.0, 0.0, 1.0]
        v3 = [1.0, 1.0, 0.0]

        file_vectors = {
            "f0": [v0, v1],       # mean = [0.5, 0.5, 0.0]
            "f1": [v2, v3],       # mean = [0.5, 0.5, 0.5]
        }

        file_ids = sorted(file_vectors.keys())
        pooled = np.array([np.mean(file_vectors[fid], axis=0) for fid in file_ids])

        self.assertEqual(pooled.shape, (2, 3))
        self.assertAlmostEqual(float(pooled[0][0]), 0.5, places=6)
        self.assertAlmostEqual(float(pooled[0][1]), 0.5, places=6)
        self.assertAlmostEqual(float(pooled[0][2]), 0.0, places=6)
        self.assertAlmostEqual(float(pooled[1][0]), 0.5, places=6)
        self.assertAlmostEqual(float(pooled[1][1]), 0.5, places=6)
        self.assertAlmostEqual(float(pooled[1][2]), 0.5, places=6)


class RefreshClustersSkipTest(unittest.TestCase):
    """When <8 files have vectors, the table is cleared and epoch set."""

    def test_less_than_8_files_clears_table(self):
        from oracle.server.query_engine import _refresh_clusters
        import oracle.config as cfg

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = Path(tmp) / "meta.sqlite"
            vec_path = Path(tmp) / "chunks.json"

            sqlite = _seed_sqlite_and_vectors(
                sqlite_path, vec_path,
                [f"src/file{i}.py" for i in range(3)],
            )

            with (
                patch.object(cfg, "SQLITE_PATH", sqlite_path),
                patch.object(cfg, "CHUNK_DB_PATH", vec_path),
            ):
                _refresh_clusters(root)

            rows = sqlite.get_file_clusters()
            epoch = sqlite.get_clusters_epoch()

            self.assertEqual(rows, [])
            self.assertIsNotNone(epoch)
            # Epoch is a content-signature hash (16 hex chars), not a timestamp.
            self.assertEqual(len(epoch), 16)
            int(epoch, 16)  # must be valid hex

    def test_zero_files_sets_epoch_and_empty(self):
        from oracle.server.query_engine import _refresh_clusters
        from oracle.store.sqlite_store import SQLiteStore
        import oracle.config as cfg

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = Path(tmp) / "meta.sqlite"
            vec_path = Path(tmp) / "chunks.json"

            sqlite = SQLiteStore(sqlite_path)

            with (
                patch.object(cfg, "SQLITE_PATH", sqlite_path),
                patch.object(cfg, "CHUNK_DB_PATH", vec_path),
            ):
                _refresh_clusters(root)

            rows = sqlite.get_file_clusters()
            epoch = sqlite.get_clusters_epoch()

            self.assertEqual(rows, [])
            self.assertIsNotNone(epoch)


class RefreshClustersKMeansTest(unittest.TestCase):
    """KMeans path (hdbscan not available) is deterministic with random_state=0."""

    def test_kmeans_produces_deterministic_clusters(self):
        from oracle.server.query_engine import _refresh_clusters
        import oracle.config as cfg

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = Path(tmp) / "meta.sqlite"
            vec_path = Path(tmp) / "chunks.json"

            # 12 files — enough for meaningful clustering
            files = [f"src/module{i}.py" for i in range(12)]
            sqlite = _seed_sqlite_and_vectors(sqlite_path, vec_path, files, chunks_per_file=3)

            with (
                patch.object(cfg, "SQLITE_PATH", sqlite_path),
                patch.object(cfg, "CHUNK_DB_PATH", vec_path),
            ):
                _refresh_clusters(root)

            rows = sqlite.get_file_clusters()
            epoch = sqlite.get_clusters_epoch()

            # All 12 files should be assigned (KMeans assigns everything)
            self.assertEqual(len(rows), 12, f"expected 12 files clustered, got {len(rows)}")
            self.assertIsNotNone(epoch)

            # KMeans with random_state=0 is deterministic — run again, same result
            sqlite.replace_file_clusters([])
            sqlite.set_clusters_epoch("")

            with (
                patch.object(cfg, "SQLITE_PATH", sqlite_path),
                patch.object(cfg, "CHUNK_DB_PATH", vec_path),
            ):
                _refresh_clusters(root)

            rows2 = sqlite.get_file_clusters()
            self.assertEqual(len(rows2), 12)

            mapping1 = {r["file_id"]: r["cluster_id"] for r in rows}
            mapping2 = {r["file_id"]: r["cluster_id"] for r in rows2}
            self.assertEqual(mapping1, mapping2,
                             "KMeans random_state=0 must be deterministic")

    def test_scores_are_between_zero_and_one(self):
        from oracle.server.query_engine import _refresh_clusters
        import oracle.config as cfg

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = Path(tmp) / "meta.sqlite"
            vec_path = Path(tmp) / "chunks.json"

            files = [f"src/file{i}.py" for i in range(10)]
            sqlite = _seed_sqlite_and_vectors(sqlite_path, vec_path, files)

            with (
                patch.object(cfg, "SQLITE_PATH", sqlite_path),
                patch.object(cfg, "CHUNK_DB_PATH", vec_path),
            ):
                _refresh_clusters(root)

            rows = sqlite.get_file_clusters()
            self.assertTrue(len(rows) > 0)

            for row in rows:
                self.assertGreaterEqual(row["score"], 0.0)
                self.assertLessEqual(row["score"], 1.0,
                                     f"score {row['score']} should be <= 1.0")

    def test_k_formula_bounds(self):
        """k = clamp(round(sqrt(n/2)), 2, 24)"""
        import math

        def k_for(n):
            return max(2, min(24, round(math.sqrt(n / 2))))

        self.assertEqual(k_for(8), 2)      # sqrt(4) = 2
        self.assertEqual(k_for(18), 3)     # sqrt(9) = 3
        self.assertEqual(k_for(32), 4)     # sqrt(16) = 4
        self.assertEqual(k_for(1200), 24)  # sqrt(600) ≈ 24.5 → 24 → clamp at 24
        self.assertEqual(k_for(2000), 24)  # clamps at 24
        self.assertEqual(k_for(5000), 24)  # clamps at 24


class ReplaceAllTransactionTest(unittest.TestCase):
    """replace_file_clusters removes old rows entirely."""

    def test_replace_all_removes_old_rows(self):
        from oracle.store.sqlite_store import SQLiteStore

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            # First write
            sqlite.replace_file_clusters([
                {"file_id": "a.py", "cluster_id": 1, "score": 0.9},
                {"file_id": "b.py", "cluster_id": 1, "score": 0.8},
                {"file_id": "c.py", "cluster_id": 2, "score": 0.7},
            ])
            rows = sqlite.get_file_clusters()
            self.assertEqual(len(rows), 3)

            # Replace with different set
            sqlite.replace_file_clusters([
                {"file_id": "x.py", "cluster_id": 0, "score": 0.5},
            ])
            rows2 = sqlite.get_file_clusters()
            self.assertEqual(len(rows2), 1)
            self.assertEqual(rows2[0]["file_id"], "x.py")

            # Replace with empty
            sqlite.replace_file_clusters([])
            rows3 = sqlite.get_file_clusters()
            self.assertEqual(rows3, [])


class ClusterMembersSortTest(unittest.TestCase):
    """get_cluster_members returns rows sorted by score desc."""

    def test_members_sorted_by_score_desc(self):
        from oracle.store.sqlite_store import SQLiteStore

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            sqlite.replace_file_clusters([
                {"file_id": "a.py", "cluster_id": 1, "score": 0.5},
                {"file_id": "b.py", "cluster_id": 1, "score": 0.9},
                {"file_id": "c.py", "cluster_id": 1, "score": 0.3},
            ])

            members = sqlite.get_cluster_members(1)
            self.assertEqual(len(members), 3)
            scores = [m["score"] for m in members]
            self.assertEqual(scores, [0.9, 0.5, 0.3])

            # Unknown cluster → empty
            empty = sqlite.get_cluster_members(999)
            self.assertEqual(empty, [])


class ResponseHelpersTest(unittest.TestCase):
    """Unit tests for _clusters_response and _cluster_members_response."""

    def test_clusters_response_shape(self):
        from oracle.store.sqlite_store import SQLiteStore
        from oracle.server.query_engine import _clusters_response

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            sqlite.set_clusters_epoch("2025-07-09T12:00:00Z")
            sqlite.replace_file_clusters([
                {"file_id": "a.py", "cluster_id": 0, "score": 0.9},
                {"file_id": "b.py", "cluster_id": 0, "score": 0.8},
                {"file_id": "c.py", "cluster_id": 0, "score": 0.7},
                {"file_id": "d.py", "cluster_id": 0, "score": 0.6},
                {"file_id": "e.py", "cluster_id": 1, "score": 0.5},
            ])

            resp = _clusters_response(sqlite)
            self.assertEqual(resp["epoch"], "2025-07-09T12:00:00Z")
            self.assertIsInstance(resp["clusters"], list)
            self.assertEqual(len(resp["clusters"]), 2)

            c0 = resp["clusters"][0]
            self.assertEqual(c0["clusterId"], 0)
            self.assertEqual(c0["size"], 4)
            self.assertEqual(len(c0["sampleFiles"]), 3)  # up to 3
            self.assertEqual(c0["sampleFiles"], ["a.py", "b.py", "c.py"])

            c1 = resp["clusters"][1]
            self.assertEqual(c1["clusterId"], 1)
            self.assertEqual(c1["size"], 1)
            self.assertEqual(c1["sampleFiles"], ["e.py"])

    def test_cluster_members_response_shape(self):
        from oracle.store.sqlite_store import SQLiteStore
        from oracle.server.query_engine import _cluster_members_response

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            sqlite.replace_file_clusters([
                {"file_id": "x.py", "cluster_id": 5, "score": 0.9},
                {"file_id": "y.py", "cluster_id": 5, "score": 0.7},
            ])

            resp = _cluster_members_response(sqlite, 5)
            self.assertEqual(resp["clusterId"], 5)
            self.assertEqual(len(resp["members"]), 2)
            self.assertEqual(resp["members"][0], {"fileId": "x.py", "score": 0.9})
            self.assertEqual(resp["members"][1], {"fileId": "y.py", "score": 0.7})

    def test_unknown_cluster_fail_open(self):
        from oracle.store.sqlite_store import SQLiteStore
        from oracle.server.query_engine import _cluster_members_response

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            resp = _cluster_members_response(sqlite, 999)
            self.assertEqual(resp["clusterId"], 999)
            self.assertEqual(resp["members"], [])


class EpochStampedTest(unittest.TestCase):
    """set_clusters_epoch writes and get_clusters_epoch reads back."""

    def test_epoch_roundtrip(self):
        from oracle.store.sqlite_store import SQLiteStore

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            self.assertIsNone(sqlite.get_clusters_epoch())

            sqlite.set_clusters_epoch("2025-07-09T15:30:00Z")
            self.assertEqual(sqlite.get_clusters_epoch(), "2025-07-09T15:30:00Z")

            # Overwrite
            sqlite.set_clusters_epoch("2025-07-10T00:00:00Z")
            self.assertEqual(sqlite.get_clusters_epoch(), "2025-07-10T00:00:00Z")


class RoutesShapeTest(unittest.TestCase):
    """FastAPI TestClient tests for /clusters and /cluster/{id}/members."""

    def test_clusters_and_members_routes(self):
        from fastapi import FastAPI, APIRouter
        from fastapi.testclient import TestClient

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "meta.sqlite"
            from oracle.store.sqlite_store import SQLiteStore
            sqlite = SQLiteStore(sqlite_path)

            sqlite.set_clusters_epoch("2025-07-09T10:00:00Z")
            sqlite.replace_file_clusters([
                {"file_id": "a.py", "cluster_id": 0, "score": 0.9},
                {"file_id": "b.py", "cluster_id": 0, "score": 0.5},
            ])

            # Build routes with no auth for testing
            router = APIRouter()

            @router.get("/clusters")
            def clusters():
                from oracle.server.query_engine import _clusters_response
                return _clusters_response(sqlite)

            @router.get("/cluster/{cluster_id}/members")
            def cluster_members(cluster_id: int):
                from oracle.server.query_engine import _cluster_members_response
                return _cluster_members_response(sqlite, cluster_id)

            app = FastAPI()
            app.include_router(router)
            client = TestClient(app)

            # Test /clusters
            resp = client.get("/clusters")
            self.assertEqual(resp.status_code, 200)
            data = resp.json()
            self.assertEqual(data["epoch"], "2025-07-09T10:00:00Z")
            self.assertEqual(len(data["clusters"]), 1)
            self.assertEqual(data["clusters"][0]["clusterId"], 0)
            self.assertEqual(data["clusters"][0]["size"], 2)

            # Test /cluster/0/members
            resp2 = client.get("/cluster/0/members")
            self.assertEqual(resp2.status_code, 200)
            data2 = resp2.json()
            self.assertEqual(data2["clusterId"], 0)
            self.assertEqual(len(data2["members"]), 2)
            self.assertEqual(data2["members"][0]["fileId"], "a.py")
            self.assertEqual(data2["members"][0]["score"], 0.9)

            # Test unknown cluster → empty, 200
            resp3 = client.get("/cluster/999/members")
            self.assertEqual(resp3.status_code, 200)
            data3 = resp3.json()
            self.assertEqual(data3["clusterId"], 999)
            self.assertEqual(data3["members"], [])

    def test_clusters_empty_when_no_data(self):
        from fastapi import FastAPI, APIRouter
        from fastapi.testclient import TestClient

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            from oracle.store.sqlite_store import SQLiteStore
            sqlite = SQLiteStore(sqlite_path)

            router = APIRouter()

            @router.get("/clusters")
            def clusters():
                from oracle.server.query_engine import _clusters_response
                return _clusters_response(sqlite)

            app = FastAPI()
            app.include_router(router)
            client = TestClient(app)

            resp = client.get("/clusters")
            self.assertEqual(resp.status_code, 200)
            data = resp.json()
            self.assertEqual(data["epoch"], "")
            self.assertEqual(data["clusters"], [])


class ReplaceFileClustersAtomicTest(unittest.TestCase):
    """M2: replace_file_clusters with epoch in a single transaction."""

    def test_replace_with_epoch_in_single_connection(self):
        from oracle.store.sqlite_store import SQLiteStore
        import sqlite3

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            # Replace with epoch — both tables change in one commit
            sqlite.replace_file_clusters(
                [{"file_id": "a.py", "cluster_id": 1, "score": 0.9}],
                epoch="2025-07-09T12:00:00Z",
            )

            # Verify both are visible immediately
            rows = sqlite.get_file_clusters()
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0]["file_id"], "a.py")
            epoch = sqlite.get_clusters_epoch()
            self.assertEqual(epoch, "2025-07-09T12:00:00Z")

            # Open a second connection directly to verify both changes are
            # visible (committed, not just in a pending transaction)
            conn = sqlite3.connect(str(sqlite_path))
            conn.execute("PRAGMA busy_timeout=5000")
            conn.execute("PRAGMA journal_mode=WAL")
            row = conn.execute(
                "SELECT file_id, cluster_id, score FROM file_clusters"
            ).fetchone()
            self.assertIsNotNone(row)
            self.assertEqual(row[0], "a.py")
            epoch_row = conn.execute(
                "SELECT value FROM clusters_meta WHERE key='epoch'"
            ).fetchone()
            self.assertIsNotNone(epoch_row)
            self.assertEqual(epoch_row[0], "2025-07-09T12:00:00Z")
            conn.close()

    def test_replace_empty_with_epoch_clears_table_sets_epoch(self):
        from oracle.store.sqlite_store import SQLiteStore

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            # Seed some data first
            sqlite.replace_file_clusters(
                [{"file_id": "x.py", "cluster_id": 0, "score": 0.5}],
                epoch="old",
            )

            # Replace with empty + new epoch
            sqlite.replace_file_clusters([], epoch="2025-07-09T13:00:00Z")

            self.assertEqual(sqlite.get_file_clusters(), [])
            self.assertEqual(sqlite.get_clusters_epoch(), "2025-07-09T13:00:00Z")

    def test_replace_without_epoch_leaves_epoch_untouched(self):
        from oracle.store.sqlite_store import SQLiteStore

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            sqlite = SQLiteStore(sqlite_path)

            sqlite.set_clusters_epoch("keep-me")
            sqlite.replace_file_clusters(
                [{"file_id": "a.py", "cluster_id": 1, "score": 0.9}],
            )
            # epoch=None -> untouched
            self.assertEqual(sqlite.get_clusters_epoch(), "keep-me")


class RefreshClustersPopulatesVectorsTest(unittest.TestCase):
    """B1: _refresh_clusters populates file_vectors.lancedb (NOT vectors.lancedb)
    so /similar works.  vectors.lancedb must NOT be created."""

    def test_similar_returns_other_file_after_refresh(self):
        from oracle.server.query_engine import _refresh_clusters, QueryEngine
        from oracle.store.lance_store import LanceStore
        import oracle.config as cfg

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = Path(tmp) / "meta.sqlite"
            chunk_vec_path = Path(tmp) / "chunks.lancedb"
            file_vec_path = Path(tmp) / "file_vectors.lancedb"
            node_vec_path = Path(tmp) / "vectors.lancedb"

            sqlite = _seed_sqlite_and_vectors(
                sqlite_path, chunk_vec_path,
                ["src/file_a.py", "src/file_b.py"],
                chunks_per_file=4,
            )

            with (
                patch.object(cfg, "SQLITE_PATH", sqlite_path),
                patch.object(cfg, "CHUNK_DB_PATH", chunk_vec_path),
                patch.object(cfg, "FILE_VECTORS_DB_PATH", file_vec_path),
            ):
                _refresh_clusters(root)

            # vectors.lancedb MUST NOT be created (would falsely activate
            # python_oracle_available() on bare deployments).
            self.assertFalse(
                node_vec_path.exists(),
                "_refresh_clusters must NOT create vectors.lancedb",
            )

            # Vectors land in file_vectors.lancedb.
            file_vectors = LanceStore(file_vec_path)
            all_records = file_vectors._read()
            ids = {r["id"] for r in all_records}
            self.assertIn("src/file_a.py", ids)
            self.assertIn("src/file_b.py", ids)

            # /similar fallback: node-card store (vectors.lancedb) is empty,
            # so similar() falls back to file_vectors and returns the other file.
            node_vectors = LanceStore(node_vec_path)
            engine = QueryEngine(
                sqlite,
                node_vectors,
                chunk_vectors=LanceStore(chunk_vec_path),
                file_vectors=file_vectors,
            )
            sim = engine.similar("src/file_a.py", limit=5)
            self.assertEqual(len(sim), 1, f"expected 1 similar result, got {len(sim)}")
            self.assertEqual(sim[0]["id"], "src/file_b.py")
            self.assertGreater(sim[0]["score"], 0.0)

    def test_two_file_fixture_preserves_distinct_vectors(self):
        """The pooled vectors for two files with different chunk vectors
        must be distinct (not collapsed to identical)."""
        from oracle.server.query_engine import _refresh_clusters
        from oracle.store.lance_store import LanceStore
        import oracle.config as cfg

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = Path(tmp) / "meta.sqlite"
            chunk_vec_path = Path(tmp) / "chunks.lancedb"
            file_vec_path = Path(tmp) / "file_vectors.lancedb"
            node_vec_path = Path(tmp) / "vectors.lancedb"

            _seed_sqlite_and_vectors(
                sqlite_path, chunk_vec_path,
                ["src/a.py", "src/b.py"],
                chunks_per_file=4,
            )

            with (
                patch.object(cfg, "SQLITE_PATH", sqlite_path),
                patch.object(cfg, "CHUNK_DB_PATH", chunk_vec_path),
                patch.object(cfg, "FILE_VECTORS_DB_PATH", file_vec_path),
            ):
                _refresh_clusters(root)

            # vectors.lancedb must not be created.
            self.assertFalse(node_vec_path.exists())

            file_vectors = LanceStore(file_vec_path)
            recs = file_vectors._read()
            self.assertEqual(len(recs), 2)
            va = next(r["vector"] for r in recs if r["id"] == "src/a.py")
            vb = next(r["vector"] for r in recs if r["id"] == "src/b.py")
            self.assertNotEqual(va, vb, "distinct files must have distinct vectors")

    def test_similar_fallback_when_node_card_store_empty(self):
        """When vectors.lancedb has no record for a node_id, similar()
        falls back to file_vectors — and returns results from there."""
        from oracle.server.query_engine import QueryEngine
        from oracle.store.lance_store import LanceStore

        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "meta.sqlite"
            chunk_vec_path = Path(tmp) / "chunks.lancedb"
            file_vec_path = Path(tmp) / "file_vectors.lancedb"
            node_vec_path = Path(tmp) / "vectors.lancedb"

            sqlite = _seed_sqlite_and_vectors(
                sqlite_path, chunk_vec_path,
                ["src/x.py", "src/y.py"],
                chunks_per_file=3,
            )

            # Populate file_vectors directly (roles-play what _refresh_clusters does).
            fv = LanceStore(file_vec_path)
            fv.replace_all([
                {"id": "src/x.py", "label": "src/x.py", "area": "file",
                 "cluster_semantic": "0", "vector": _make_distinct_chunk_vector(0, 0)},
                {"id": "src/y.py", "label": "src/y.py", "area": "file",
                 "cluster_semantic": "0", "vector": _make_distinct_chunk_vector(1, 0)},
            ])

            # Node-card store is empty (never populated).
            node_vectors = LanceStore(node_vec_path)
            engine = QueryEngine(
                sqlite, node_vectors,
                chunk_vectors=LanceStore(chunk_vec_path),
                file_vectors=fv,
            )
            # similar("src/x.py") should find src/y.py via file_vectors fallback.
            sim = engine.similar("src/x.py", limit=5)
            self.assertEqual(len(sim), 1)
            self.assertEqual(sim[0]["id"], "src/y.py")



if __name__ == "__main__":
    unittest.main()
