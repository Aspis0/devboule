import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from oracle.bootstrap.ingest_legacy_graph import ingest


class OracleCliTest(unittest.TestCase):
    def test_cli_emits_rust_tauri_shapes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            graph_path = root / "graph.json"
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"
            graph_path.write_text(
                json.dumps(
                    {
                        "nodes": [
                            {
                                "id": "src-tauri/src/backend/commands.rs",
                                "label": "commands.rs",
                                "cluster": 1,
                                "community": "Cloudflare",
                                "metadata": {
                                    "docstring": "Cloudflare Worker secret rotation command backend.",
                                    "dependencies": ["src-tauri/src/backend/providers.rs"],
                                },
                            },
                            {
                                "id": "src-tauri/src/backend/providers.rs",
                                "label": "providers.rs",
                                "cluster": 1,
                                "community": "Cloudflare",
                                "metadata": {
                                    "docstring": "Cloudflare Workers inventory and secret rotation provider adapter.",
                                    "dependencies": [],
                                },
                            },
                            {
                                "id": "src/components/dashboard/WorkersTable.tsx",
                                "label": "WorkersTable.tsx",
                                "cluster": 6,
                                "community": "Browser",
                                "metadata": {
                                    "docstring": "Frontend worker table.",
                                    "dependencies": [],
                                },
                            },
                        ],
                        "edges": [],
                    }
                ),
                encoding="utf-8",
            )
            ingest(
                graph_path=graph_path,
                sqlite_path=sqlite_path,
                vector_path=vector_path,
                use_sentence_transformer=False,
            )

            env = {"ORACLE_ASK_DISABLE_LLM": "1", "ORACLE_QUERY_EMBEDDER": "hash"}
            snapshot = self._run_cli(root, "snapshot", sqlite_path, vector_path, env=env)
            self.assertEqual(snapshot["source"], "python-oracle")
            self.assertEqual(snapshot["phase"], "phase1-python")
            self.assertEqual(snapshot["node_count"], 3)

            answer = self._run_cli(
                root,
                "ask",
                sqlite_path,
                vector_path,
                "--query",
                "cloudflare worker secret rotation",
                "--limit",
                "1",
                env=env,
            )
            self.assertEqual(answer["mode"], "python-oracle")
            self.assertIn(
                answer["results"][0]["id"],
                {"src-tauri/src/backend/commands.rs", "src-tauri/src/backend/providers.rs"},
            )
            self.assertIn("file_source", answer["results"][0])
            self.assertIn("function_primary", answer["results"][0])

            node = self._run_cli(
                root,
                "node",
                sqlite_path,
                vector_path,
                "--node-id",
                "src-tauri/src/backend/commands.rs",
                env=env,
            )
            self.assertEqual(node["source"], "legacy-graph")
            self.assertIn("used_by", node)

    def _run_cli(self, cwd: Path, command: str, sqlite: Path, vectors: Path, *extra: str, env: dict[str, str] | None = None) -> dict:
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "oracle.cli",
                command,
                "--sqlite",
                str(sqlite),
                "--vectors",
                str(vectors),
                *extra,
            ],
            cwd=Path.cwd(),
            capture_output=True,
            text=True,
            check=True,
            env={**os.environ, **(env or {})},
        )
        return json.loads(result.stdout)


if __name__ == "__main__":
    unittest.main()
