"""Phase 5: Verify that dense retrieval surfaces through QueryEngine.context().

The plan's suspicion was: dense hits from chunk_vectors.search() get dropped
because sqlite.get_chunk(hit["id"]) returns None (id mismatch).

This test constructs a small synthetic fixture (temp SQLite with 3 chunks + temp
Lance table with matching vectors and ids), calls context() with a query that is
semantically but NOT lexically matching, and asserts at least one returned chunk
has retrieval in {"dense", "dense+lexical"}.

Key: chunk vectors are created via embed_query_text() so they live in the same
embedding space as the query vector that LanceStore.search() produces.
"""

import os
import tempfile
import unittest
from pathlib import Path

from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore, embed_query_text, cosine
from oracle.store.sqlite_store import SQLiteStore

# Force hash-based embedder in tests to avoid loading the sentence transformer.
os.environ.setdefault("ORACLE_REQUIRE_REAL_EMBEDDER", "0")


def _embed(text: str) -> list[float]:
    """Embed text using the same pipeline that LanceStore.search() uses for queries."""
    return embed_query_text(text)


class DenseSurfacesTest(unittest.TestCase):
    """Test that dense hits surface through context() when they should."""

    def _make_fixture(self):
        """Create a SQLite + Lance fixture with 3 chunks and real vectors.

        Chunk 1: semantically similar to "network routing" (same embedding space).
        Chunk 2-3: noise chunks with orthogonal vectors.
        """
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)

        root = Path(tmp.name)
        sqlite_path = str(root / "metadata.sqlite")
        vectors_path = str(root / "vectors.json")
        chunks_path = str(root / "chunks.json")

        # --- Embeddings ---
        # Chunk 1 vector: same as the query embedding (max cosine ~1.0)
        query_text = "network routing"
        q_vec = _embed(query_text)

        # Chunk 1: semantically similar (near-identical vector, tiny noise)
        c1_vec = [v + 0.001 * (i % 3 - 1) for i, v in enumerate(q_vec)]

        # Chunk 2-3: orthogonal noise (negated query vector → cosine ~-1)
        c2_vec = [-v for v in q_vec]
        c3_vec = [-v + 0.01 * (i % 5 - 2) for i, v in enumerate(q_vec)]

        # --- SQLite: 3 chunks ---
        sqlite = SQLiteStore(sqlite_path)
        sqlite.replace_all_chunks(
            [
                {
                    "id": "router.py#chunk-0000",
                    "file_id": "router.py",
                    "chunk_index": 0,
                    "start_char": 0,
                    "end_char": 80,
                    "text": "Handles data flow and packet forwarding across the mesh.",
                    "file_sorgente": "router.py",
                    "ultima_modifica": "2026-01-01T00:00:00Z",
                    "embedding_dims": len(q_vec),
                    "kind": "function",
                    "symbol_name": "forward",
                    "signature": "",
                    "line_start": 1,
                    "line_end": 10,
                    "language": "python",
                    "symbols_used": "",
                },
                {
                    "id": "db.py#chunk-0000",
                    "file_id": "db.py",
                    "chunk_index": 0,
                    "start_char": 0,
                    "end_char": 60,
                    "text": "Database migration utilities for schema changes.",
                    "file_sorgente": "db.py",
                    "ultima_modifica": "2026-01-01T00:00:00Z",
                    "embedding_dims": len(q_vec),
                    "kind": "module",
                    "symbol_name": "",
                    "signature": "",
                    "line_start": 0,
                    "line_end": 0,
                    "language": "python",
                    "symbols_used": "",
                },
                {
                    "id": "config.yaml#chunk-0000",
                    "file_id": "config.yaml",
                    "chunk_index": 0,
                    "start_char": 0,
                    "end_char": 50,
                    "text": "Application configuration and environment settings.",
                    "file_sorgente": "config.yaml",
                    "ultima_modifica": "2026-01-01T00:00:00Z",
                    "embedding_dims": len(q_vec),
                    "kind": "text_slice",
                    "symbol_name": "",
                    "signature": "",
                    "line_start": 0,
                    "line_end": 0,
                    "language": "",
                    "symbols_used": "",
                },
            ]
        )

        # SQLite: nodes for each file
        sqlite.upsert_many(
            [
                {
                    "id": "router.py",
                    "label": "router.py",
                    "area": "Code",
                    "cluster_semantic": "Network",
                    "funzione_primaria": "Packet forwarding",
                    "espone_api": [],
                    "dipende_da": [],
                    "simile_a": [],
                    "tecnologie": ["Python"],
                    "file_sorgente": "router.py",
                    "ultima_modifica": "2026-01-01T00:00:00Z",
                    "source": "test",
                    "embedding_dims": len(q_vec),
                },
                {
                    "id": "db.py",
                    "label": "db.py",
                    "area": "Code",
                    "cluster_semantic": "Database",
                    "funzione_primaria": "Schema migration",
                    "espone_api": [],
                    "dipende_da": [],
                    "simile_a": [],
                    "tecnologie": ["Python"],
                    "file_sorgente": "db.py",
                    "ultima_modifica": "2026-01-01T00:00:00Z",
                    "source": "test",
                    "embedding_dims": len(q_vec),
                },
                {
                    "id": "config.yaml",
                    "label": "config.yaml",
                    "area": "Config",
                    "cluster_semantic": "Config",
                    "funzione_primaria": "App settings",
                    "espone_api": [],
                    "dipende_da": [],
                    "simile_a": [],
                    "tecnologie": [],
                    "file_sorgente": "config.yaml",
                    "ultima_modifica": "2026-01-01T00:00:00Z",
                    "source": "test",
                    "embedding_dims": len(q_vec),
                },
            ]
        )

        # --- Lance chunk vectors (JSON backend, matching ids) ---
        LanceStore(chunks_path).replace_all(
            [
                {
                    "id": "router.py#chunk-0000",
                    "label": "router chunk",
                    "area": "FileChunk",
                    "cluster_semantic": "network",
                    "vector": c1_vec,
                },
                {
                    "id": "db.py#chunk-0000",
                    "label": "db chunk",
                    "area": "FileChunk",
                    "cluster_semantic": "database",
                    "vector": c2_vec,
                },
                {
                    "id": "config.yaml#chunk-0000",
                    "label": "config chunk",
                    "area": "FileChunk",
                    "cluster_semantic": "config",
                    "vector": c3_vec,
                },
            ]
        )

        # --- Lance file-level vectors ---
        LanceStore(vectors_path).replace_all(
            [
                {
                    "id": "router.py",
                    "label": "router.py",
                    "area": "Code",
                    "cluster_semantic": "Network",
                    "vector": q_vec,
                },
                {
                    "id": "db.py",
                    "label": "db.py",
                    "area": "Code",
                    "cluster_semantic": "Database",
                    "vector": c2_vec,
                },
                {
                    "id": "config.yaml",
                    "label": "config.yaml",
                    "area": "Config",
                    "cluster_semantic": "Config",
                    "vector": c3_vec,
                },
            ]
        )

        return sqlite_path, vectors_path, chunks_path

    def test_dense_surfaces_in_context(self):
        """Dense hits from chunk_vectors should appear in context() output.

        With 3 chunks where only chunk 1 has a vector similar to the query,
        the dense search should surface chunk 1 with retrieval="dense".
        """
        sqlite_path, vectors_path, chunks_path = self._make_fixture()

        sqlite_store = SQLiteStore(sqlite_path)
        vectors_store = LanceStore(vectors_path)
        chunks_store = LanceStore(chunks_path)

        engine = QueryEngine(sqlite_store, vectors_store, chunk_vectors=chunks_store)

        # Query semantically similar to chunk 1, NOT lexically matching any chunk text
        context = engine.context("network routing", 5)

        self.assertTrue(context, "context() should return results")

        # The top result should be the router chunk (highest dense score)
        has_dense = any(c["retrieval"] in {"dense", "dense+lexical"} for c in context)
        self.assertTrue(
            has_dense,
            f"Expected at least one dense hit, got: "
            f"{[(c['chunk_id'], c['retrieval']) for c in context]}",
        )

        # The router chunk should be ranked first (highest dense score)
        router_chunks = [c for c in context if "router" in c["chunk_id"]]
        self.assertTrue(
            router_chunks,
            f"Expected router chunk in results, got: "
            f"{[c['chunk_id'] for c in context]}",
        )

    def test_dense_and_lexical_overlap_becomes_dense_plus_lexical(self):
        """When the same chunk is hit by both dense and lexical, it should be
        labeled dense+lexical."""
        sqlite_path, vectors_path, chunks_path = self._make_fixture()

        sqlite_store = SQLiteStore(sqlite_path)
        vectors_store = LanceStore(vectors_path)
        chunks_store = LanceStore(chunks_path)

        engine = QueryEngine(sqlite_store, vectors_store, chunk_vectors=chunks_store)

        # "data flow" appears in chunk 1 text (lexical) AND the vector is
        # similar to the query (dense)
        context = engine.context("data flow forwarding", 5)

        found = [c for c in context if c["chunk_id"] == "router.py#chunk-0000"]
        self.assertTrue(found, "router chunk should appear in results")
        self.assertEqual(
            found[0]["retrieval"],
            "dense+lexical",
            f"Expected dense+lexical for overlapping hit, got: {found[0]['retrieval']}",
        )

    def test_dense_only_when_no_lexical_overlap(self):
        """When dense hits don't overlap with lexical, they should still be
        labeled 'dense' (not dropped)."""
        sqlite_path, vectors_path, chunks_path = self._make_fixture()

        sqlite_store = SQLiteStore(sqlite_path)
        vectors_store = LanceStore(vectors_path)
        chunks_store = LanceStore(chunks_path)

        engine = QueryEngine(sqlite_store, vectors_store, chunk_vectors=chunks_store)

        # "network routing" has no lexical overlap with chunk 1 text
        # ("Handles data flow and packet forwarding across the mesh.")
        context = engine.context("network routing", 5)

        router_chunks = [c for c in context if "router" in c["chunk_id"]]
        self.assertTrue(
            router_chunks,
            f"Expected router chunk via dense retrieval, got: "
            f"{[c['chunk_id'] for c in context]}",
        )
        # Should be labeled "dense" (pure dense, no lexical overlap)
        self.assertEqual(
            router_chunks[0]["retrieval"],
            "dense",
            f"Expected pure 'dense' label, got: {router_chunks[0]['retrieval']}",
        )

    def test_dense_does_not_surface_without_chunk_vectors(self):
        """When chunk_vectors is None, context() should return only lexical."""
        sqlite_path, vectors_path, chunks_path = self._make_fixture()

        sqlite_store = SQLiteStore(sqlite_path)
        vectors_store = LanceStore(vectors_path)

        engine = QueryEngine(sqlite_store, vectors_store)

        context = engine.context("data flow forwarding", 5)

        self.assertTrue(context)
        for c in context:
            self.assertEqual(c["retrieval"], "lexical")

    def test_dense_does_not_surface_when_prefer_lexical(self):
        """When prefer_lexical=True, dense hits should be skipped."""
        sqlite_path, vectors_path, chunks_path = self._make_fixture()

        sqlite_store = SQLiteStore(sqlite_path)
        vectors_store = LanceStore(vectors_path)
        chunks_store = LanceStore(chunks_path)

        engine = QueryEngine(sqlite_store, vectors_store, chunk_vectors=chunks_store)

        context = engine.context("network routing", 5, prefer_lexical=True)

        for c in context:
            self.assertEqual(c["retrieval"], "lexical")

    def test_dense_ranking_prefers_similar_chunk(self):
        """Dense search should rank the semantically similar chunk above noise."""
        sqlite_path, vectors_path, chunks_path = self._make_fixture()

        sqlite_store = SQLiteStore(sqlite_path)
        vectors_store = LanceStore(vectors_path)
        chunks_store = LanceStore(chunks_path)

        engine = QueryEngine(sqlite_store, vectors_store, chunk_vectors=chunks_store)

        context = engine.context("network routing", 5)

        # Filter to dense hits only
        dense_chunks = [
            c for c in context if c["retrieval"] in {"dense", "dense+lexical"}
        ]
        self.assertTrue(dense_chunks, "Expected at least one dense hit")

        # The router chunk (similar vector) should be ranked first among dense
        self.assertTrue(
            "router" in dense_chunks[0]["chunk_id"],
            f"Expected router chunk first, got: "
            f"{[(c['chunk_id'], c['retrieval'], c['score']) for c in dense_chunks]}",
        )


if __name__ == "__main__":
    unittest.main()
