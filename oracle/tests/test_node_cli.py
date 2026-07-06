"""Phase 3 — `oracle node` empty/unknown id fail gracefully (TDD).

Verifies that:
  - Empty --node-id returns {"error": "node_id required"} with non-zero exit.
  - Unknown --node-id returns {"error": "not_found", "node_id": "<id>"} instead of
    an uncaught KeyError traceback.

Happy-path (valid node_id) is left unchanged.
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from oracle.store.sqlite_store import SQLiteStore


class TestNodeCliGracefulFailure(unittest.TestCase):
    """Tests for graceful failure on empty/unknown node IDs."""

    def _seed_node_cards(self, sqlite_path: Path) -> str:
        """Seed a temp SQLite with one node_cards row and return its id."""
        store = SQLiteStore(sqlite_path)
        store.upsert_many(
            [
                {
                    "id": "src-tauri/src/main.rs",
                    "label": "main.rs",
                    "area": "Rust/Tauri",
                    "cluster_semantic": "rust-tauri",
                    "funzione_primaria": "Application entry point",
                    "espone_api": "none",
                    "dipende_da": json.dumps(["src-tauri/src/app.rs"]),
                    "simile_a": json.dumps([]),
                    "tecnologie": json.dumps(["rust", "tauri"]),
                    "file_sorgente": "src-tauri/src/main.rs",
                    "ultima_modifica": "2026-07-01T00:00:00Z",
                    "source": "build-cards",
                    "embedding_dims": 1024,
                }
            ]
        )
        return "src-tauri/src/main.rs"

    def _run_cli(self, tmp: Path, *extra_args: str) -> subprocess.CompletedProcess:
        """Run `python -m oracle.cli node` and return the CompletedProcess."""
        sqlite = tmp / "metadata.sqlite"
        vectors = tmp / "vectors.json"
        return subprocess.run(
            [
                sys.executable,
                "-m",
                "oracle.cli",
                "node",
                "--sqlite",
                str(sqlite),
                "--vectors",
                str(vectors),
                *extra_args,
            ],
            capture_output=True,
            text=True,
        )

    def test_empty_node_id_returns_error_dict_nonzero_exit(self):
        """Empty --node-id → {"error": "node_id required"}, exit != 0."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            SQLiteStore(sqlite_path)  # create schema

            result = self._run_cli(root, "--node-id", "")
            self.assertNotEqual(
                result.returncode, 0, "Expected non-zero exit for empty node_id"
            )
            error = json.loads(result.stdout)
            self.assertEqual(error["error"], "node_id required")

    def test_unknown_node_id_returns_not_found(self):
        """Unknown --node-id → {"error": "not_found", "node_id": "..."}, no traceback."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            valid_id = self._seed_node_cards(sqlite_path)

            result = self._run_cli(root, "--node-id", "non-existent-file.rs")
            # Exit code 0 (structured error, not crash), no traceback in stderr
            self.assertEqual(result.returncode, 0)
            error = json.loads(result.stdout)
            self.assertEqual(error["error"], "not_found")
            self.assertEqual(error["node_id"], "non-existent-file.rs")
            self.assertNotIn("Traceback", result.stderr)

    def test_valid_node_id_still_works(self):
        """Happy path: valid node_id returns full card, unchanged."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            valid_id = self._seed_node_cards(sqlite_path)

            result = self._run_cli(root, "--node-id", valid_id)
            self.assertEqual(result.returncode, 0)
            node = json.loads(result.stdout)
            self.assertEqual(node["id"], valid_id)
            self.assertEqual(node["label"], "main.rs")


if __name__ == "__main__":
    unittest.main()
