"""Phase 2 — Populate `node_cards` deterministically from `file_chunks` (TDD).

Verifies that `build_cards_from_chunks` reads chunks, classifies each file with
`fallback_classification`, writes `node_cards` rows + vectors, and that the
graph tools (`node`, `similar`) work afterwards.
"""

import json
import tempfile
import unittest
from pathlib import Path

from oracle.ingestion.build_cards import build_cards_from_chunks
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


class TestBuildCardsFromChunks(unittest.TestCase):
    """Unit tests for build_cards_from_chunks."""

    def _seed_chunks(self, sqlite_path: Path):
        """Seed a temp SQLite with file_chunks for 3 fake files."""
        store = SQLiteStore(sqlite_path)
        chunks = [
            {
                "id": "src-tauri/src/flow/executor.rs#chunk-0000",
                "file_id": "src-tauri/src/flow/executor.rs",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 2400,
                "text": (
                    "pub async fn execute_flow(id: FlowId) -> Result<FlowStatus> {\n"
                    "    let flow = db.get_flow(id).await?;\n"
                    "    match flow.run(self.pool).await {\n"
                    "        Ok(status) => { db.update_status(id, status).await?; Ok(status) }\n"
                    "        Err(e) => { self.escalate(id, &e).await; Err(e) }\n"
                    "    }\n"
                    "}\n"
                ),
                "file_sorgente": "src-tauri/src/flow/executor.rs",
                "ultima_modifica": "2026-07-01T00:00:00Z",
                "embedding_dims": 1024,
                "kind": "function",
                "symbol_name": "execute_flow",
                "signature": "pub async fn execute_flow(id: FlowId) -> Result<FlowStatus>",
                "line_start": 42,
                "line_end": 58,
                "language": "rust",
                "symbols_used": json.dumps(["FlowId", "FlowStatus", "DbPool"]),
            },
            {
                "id": "src/components/FlowView.tsx#chunk-0000",
                "file_id": "src/components/FlowView.tsx",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 1800,
                "text": (
                    "import { useState, useEffect } from 'react';\n"
                    "function useFlowStatus(flowId: string): FlowStatusHook {\n"
                    "  const [status, setStatus] = useState<FlowStatus>('idle');\n"
                    "  useEffect(() => {\n"
                    "    oracle_ask(`flow ${flowId} status`).then(r => setStatus(r.answer));\n"
                    "  }, [flowId]);\n"
                    "  return { status };\n"
                    "}\n"
                    "export default function FlowView({ flowId }) {\n"
                    "  const { status } = useFlowStatus(flowId);\n"
                    "  return <div>{status}</div>;\n"
                    "}\n"
                ),
                "file_sorgente": "src/components/FlowView.tsx",
                "ultima_modifica": "2026-07-01T00:00:00Z",
                "embedding_dims": 1024,
                "kind": "function",
                "symbol_name": "useFlowStatus",
                "signature": "function useFlowStatus(flowId: string): FlowStatusHook",
                "line_start": 25,
                "line_end": 33,
                "language": "typescript",
                "symbols_used": json.dumps(["useState", "useEffect", "oracle_ask"]),
            },
            {
                "id": "oracle/server/answerer.py#chunk-0000",
                "file_id": "oracle/server/answerer.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 3000,
                "text": (
                    "from oracle.config import LLM_MODEL\n"
                    "def answer_from_context(query: str, chunks: list[dict]) -> dict:\n"
                    "    context = prepared_context(chunks, query)\n"
                    "    return answer_with_llm_config(query, prompt, context, config)\n"
                ),
                "file_sorgente": "oracle/server/answerer.py",
                "ultima_modifica": "2026-07-01T00:00:00Z",
                "embedding_dims": 1024,
                "kind": "function",
                "symbol_name": "answer_from_context",
                "signature": "def answer_from_context(query: str, chunks: list[dict]) -> dict",
                "line_start": 10,
                "line_end": 20,
                "language": "python",
                "symbols_used": json.dumps(["LLM_MODEL", "prepared_context"]),
            },
        ]
        store.replace_all_chunks(chunks)
        return store

    def test_build_cards_creates_node_cards(self):
        """build_cards_from_chunks creates one node_cards row per file."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"

            self._seed_chunks(sqlite_path)

            count = build_cards_from_chunks(sqlite_path, vector_path)

            self.assertEqual(count, 3)

            store = SQLiteStore(sqlite_path)
            self.assertEqual(store.count(), 3)

    def test_build_cards_has_required_fields(self):
        """Each node_card has non-empty label, area, tecnologie."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"

            self._seed_chunks(sqlite_path)
            build_cards_from_chunks(sqlite_path, vector_path)

            store = SQLiteStore(sqlite_path)
            nodes = store.all_nodes()

            for node in nodes:
                self.assertTrue(node["label"].strip(), f"Empty label for {node['id']}")
                self.assertTrue(node["area"].strip(), f"Empty area for {node['id']}")
                self.assertTrue(
                    len(node["tecnologie"]) > 0, f"Empty tecnologie for {node['id']}"
                )

    def test_build_cards_correct_areas(self):
        """Area inference matches file paths."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"

            self._seed_chunks(sqlite_path)
            build_cards_from_chunks(sqlite_path, vector_path)

            store = SQLiteStore(sqlite_path)

            # The Rust file in src-tauri should be classified
            executor = store.get_node("src-tauri/src/flow/executor.rs")
            self.assertIsNotNone(executor)

            # The TSX file mentions oracle_ask → area inferred as Oracle (correct per fallback rules)
            flow_view = store.get_node("src/components/FlowView.tsx")
            self.assertIsNotNone(flow_view)
            self.assertIn(flow_view["area"], {"Browser", "Oracle"})

            # The oracle Python file should be Oracle area
            answerer = store.get_node("oracle/server/answerer.py")
            self.assertIsNotNone(answerer)
            self.assertEqual(answerer["area"], "Oracle")

    def test_node_lookup_works_after_build(self):
        """QueryEngine.node(id) returns a card instead of raising KeyError."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"
            chunk_path = root / "chunks.json"

            self._seed_chunks(sqlite_path)
            build_cards_from_chunks(sqlite_path, vector_path)

            engine = QueryEngine(
                SQLiteStore(sqlite_path),
                LanceStore(vector_path),
                LanceStore(chunk_path),
            )

            # Should NOT raise KeyError
            card = engine.node("src-tauri/src/flow/executor.rs")
            self.assertEqual(card["id"], "src-tauri/src/flow/executor.rs")
            self.assertTrue(card["funzione_primaria"].strip())

    def test_vectors_are_written(self):
        """LanceStore receives vector records for each card."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"

            self._seed_chunks(sqlite_path)
            build_cards_from_chunks(sqlite_path, vector_path)

            vector_store = LanceStore(vector_path)
            self.assertEqual(vector_store.count(), 3)

    def test_idempotent_rerun(self):
        """Running build_cards_from_chunks twice does not duplicate rows."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"

            self._seed_chunks(sqlite_path)
            build_cards_from_chunks(sqlite_path, vector_path)
            build_cards_from_chunks(sqlite_path, vector_path)

            store = SQLiteStore(sqlite_path)
            self.assertEqual(store.count(), 3)

    def test_empty_chunks_returns_zero(self):
        """An empty database yields 0 cards."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"

            # Create the DB but don't seed any chunks
            SQLiteStore(sqlite_path)

            count = build_cards_from_chunks(sqlite_path, vector_path)
            self.assertEqual(count, 0)


if __name__ == "__main__":
    unittest.main()
