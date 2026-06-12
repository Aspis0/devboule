import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from oracle.bootstrap.ingest_legacy_graph import ingest
from oracle.ingestion.chunk_index import (
    chunk_path_allowed,
    collect_text_files,
    dir_is_install_root,
    prune_excluded_chunks,
)
from oracle.ingestion.parser import is_sensitive_relative_path
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


class OraclePhase01Test(unittest.TestCase):
    def test_ingest_legacy_graph_populates_sqlite_and_vector_store(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            graph_path = self._write_graph(root)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"

            imported = ingest(
                graph_path=graph_path,
                sqlite_path=sqlite_path,
                vector_path=vector_path,
                use_sentence_transformer=False,
            )

            self.assertEqual(imported, 5)
            store = SQLiteStore(sqlite_path)
            card = store.get_node("workers/android/auth.js")
            self.assertIsNotNone(card)
            self.assertEqual(card["source"], "legacy-graph")
            self.assertEqual(card["embedding_dims"], 1024)
            self.assertEqual(card["area"], "CF-Android")
            self.assertIn("POST /auth/refresh", card["espone_api"])

            vector_store = LanceStore(vector_path)
            self.assertEqual(vector_store.count(), 5)

    def test_reingest_replaces_stale_nodes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            graph_path = self._write_graph(root)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"
            ingest(
                graph_path=graph_path,
                sqlite_path=sqlite_path,
                vector_path=vector_path,
                use_sentence_transformer=False,
            )
            graph_path.write_text(
                json.dumps(
                    {
                        "nodes": [
                            {
                                "id": "src/App.tsx",
                                "label": "App.tsx",
                                "cluster": "Frontend",
                                "metadata": {"docstring": "React app shell."},
                            }
                        ],
                        "edges": [],
                    }
                ),
                encoding="utf-8",
            )

            imported = ingest(
                graph_path=graph_path,
                sqlite_path=sqlite_path,
                vector_path=vector_path,
                use_sentence_transformer=False,
            )

            self.assertEqual(imported, 1)
            self.assertEqual(SQLiteStore(sqlite_path).count(), 1)
            self.assertEqual(LanceStore(vector_path).count(), 1)
            self.assertIsNone(SQLiteStore(sqlite_path).get_node("workers/android/auth.js"))

    def test_prune_excluded_chunks_removes_stale_node_cards_and_vectors(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / "keep.py").write_text("print('keep')\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vector_path = root / "chunks.json"
            node_vector_path = root / "vectors.json"
            manifest_path = root / "manifest.json"
            sqlite = SQLiteStore(sqlite_path)
            sqlite.upsert_many(
                [
                    self._card("keep.py"),
                    self._card("oracle/bootstrap/deleted_current_app.py"),
                ]
            )
            LanceStore(node_vector_path).replace_all(
                [
                    {"id": "keep.py", "label": "keep.py", "vector": [1.0, 0.0]},
                    {
                        "id": "oracle/bootstrap/deleted_current_app.py",
                        "label": "deleted_current_app.py",
                        "vector": [0.0, 1.0],
                    },
                ]
            )

            result = prune_excluded_chunks(
                root,
                sqlite_path,
                chunk_vector_path,
                manifest_path,
                node_vector_path=node_vector_path,
            )

            self.assertEqual(result["removed_nodes"], 1)
            self.assertEqual(result["removed_node_vectors"], 1)
            self.assertIsNotNone(SQLiteStore(sqlite_path).get_node("keep.py"))
            self.assertIsNone(
                SQLiteStore(sqlite_path).get_node("oracle/bootstrap/deleted_current_app.py")
            )
            self.assertEqual(LanceStore(node_vector_path).ids(), ["keep.py"])

    def test_collect_text_files_excludes_baseline_copies(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / "src").mkdir()
            (root / "src" / "live.py").write_text("print('live')\n", encoding="utf-8")
            (root / "_baseline").mkdir()
            (root / "_baseline" / "old.py").write_text("print('old')\n", encoding="utf-8")

            files = {path.relative_to(root).as_posix() for path in collect_text_files(root)}

            self.assertIn("src/live.py", files)
            self.assertNotIn("_baseline/old.py", files)

    def test_collect_text_files_respects_workspace_oracleignore(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / ".oracleignore").write_text(
                "local-cache/\n**/node_modules/\n*.secret.txt\n",
                encoding="utf-8",
            )
            (root / "src").mkdir()
            (root / "src" / "live.py").write_text("print('live')\n", encoding="utf-8")
            (root / "local-cache").mkdir()
            (root / "local-cache" / "ignored.py").write_text("print('ignored')\n", encoding="utf-8")
            (root / "app" / "node_modules").mkdir(parents=True)
            (root / "app" / "node_modules" / "pkg.js").write_text("export const ignored = true;\n", encoding="utf-8")
            (root / "notes.secret.txt").write_text("token-like text\n", encoding="utf-8")

            files = {path.relative_to(root).as_posix() for path in collect_text_files(root)}

            self.assertEqual(files, {"src/live.py"})

    def test_collect_text_files_rejects_secret_files_keeps_source_tokens(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / "src").mkdir()
            # Legitimate source named tokens.ts must survive (README carve-out).
            (root / "src" / "tokens.ts").write_text("export const A = 1;\n", encoding="utf-8")
            (root / "src" / "tokens.py").write_text("A = 1\n", encoding="utf-8")
            # Secret dumps that must NEVER be indexed.
            (root / "token.txt").write_text("ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n", encoding="utf-8")
            (root / "tokens.txt").write_text("scw-secret\n", encoding="utf-8")
            (root / "secrets.yaml").write_text("api_key: x\n", encoding="utf-8")
            (root / "secrets.yml").write_text("api_key: x\n", encoding="utf-8")
            (root / "credentials.yaml").write_text("user: x\n", encoding="utf-8")
            (root / "config").mkdir()
            (root / "config" / "secrets.toml").write_text("a = 1\n", encoding="utf-8")
            (root / "api-keys.json").write_text("{}\n", encoding="utf-8")
            (root / "creds.txt").write_text("user:pass\n", encoding="utf-8")
            (root / "vault.json").write_text("{}\n", encoding="utf-8")
            (root / "aspis-secrets").mkdir()
            (root / "aspis-secrets" / "keys.txt").write_text("secret\n", encoding="utf-8")
            (root / ".env.production").write_text("KEY=value\n", encoding="utf-8")

            files = {path.relative_to(root).as_posix() for path in collect_text_files(root)}

            self.assertIn("src/tokens.ts", files)
            self.assertIn("src/tokens.py", files)
            for blocked in (
                "token.txt",
                "tokens.txt",
                "secrets.yaml",
                "secrets.yml",
                "credentials.yaml",
                "config/secrets.toml",
                "api-keys.json",
                "creds.txt",
                "vault.json",
                "aspis-secrets/keys.txt",
                ".env.production",
            ):
                self.assertNotIn(blocked, files, f"{blocked} must not be indexed")

    def test_builtin_deny_list_blocks_secrets_without_ignore_file(self):
        # C2: the built-in default-deny filter must block the documented secret
        # set on its own, with NO .oracleignore present. chunk_path_allowed reads
        # no ignore file, proving the security invariant is not user-droppable.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            self.assertFalse((root / ".oracleignore").exists())
            blocked = [
                "token.txt",
                "tokens.txt",
                "secrets.yaml",
                "secrets.yml",
                "credentials.yaml",
                "config/secrets.toml",
                "api-keys.json",
                "creds.txt",
                "vault.json",
                "aspis-secrets/keys.txt",
                ".env",
                ".env.local",
                "deploy/server.key",
                "tls/cert.pem",
            ]
            for rel in blocked:
                self.assertTrue(
                    is_sensitive_relative_path(rel),
                    f"{rel} should be denied by the built-in filter",
                )
                self.assertFalse(
                    chunk_path_allowed(root / rel, root, ignore_policy={}),
                    f"{rel} should be rejected by chunk_path_allowed without ignore file",
                )
            for allowed in ("src/tokens.ts", "src/tokens.py", "src/tokens.tsx", "src/app.py"):
                self.assertFalse(
                    is_sensitive_relative_path(allowed),
                    f"{allowed} is legitimate source and must be allowed",
                )
                self.assertTrue(
                    chunk_path_allowed(root / allowed, root, ignore_policy={}),
                    f"{allowed} should be accepted by chunk_path_allowed",
                )

    def test_query_engine_exposes_phase1_routes_semantics(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            graph_path = self._write_graph(root)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"
            ingest(
                graph_path=graph_path,
                sqlite_path=sqlite_path,
                vector_path=vector_path,
                use_sentence_transformer=False,
            )

            engine = QueryEngine(SQLiteStore(sqlite_path), LanceStore(vector_path))

            with patch.dict("os.environ", {"ORACLE_ASK_DISABLE_LLM": "1", "ORACLE_QUERY_EMBEDDER": "hash"}):
                answer = engine.ask("jwt token refresh", limit=2)
                self.assertEqual(answer["mode"], "oracle-qwen-local")
                self.assertEqual(answer["results"][0]["id"], "workers/android/auth.js")

                scaleway = engine.ask("scaleway gpu inference", limit=1)
                self.assertEqual(scaleway["results"][0]["id"], "containers/scaleway/gpu-worker.py")

                cloudflare = engine.ask("cloudflare worker secret rotation", limit=1)
                self.assertEqual(cloudflare["results"][0]["id"], "src-tauri/src/backend/providers.rs")

            similar = engine.similar("workers/android/auth.js", limit=2)
            self.assertEqual(similar[0]["id"], "workers/browser/auth.js")

            duplicates = engine.duplicates()
            self.assertEqual(duplicates[0], ["workers/android/auth.js", "workers/browser/auth.js"])

    def test_app_graph_cloudflare_secret_query_prefers_backend_provider_code(self):
        app_graph = Path("graph.json")
        if not app_graph.exists():
            self.skipTest("app graph.json not available")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"
            ingest(
                graph_path=app_graph,
                sqlite_path=sqlite_path,
                vector_path=vector_path,
                use_sentence_transformer=False,
            )
            engine = QueryEngine(SQLiteStore(sqlite_path), LanceStore(vector_path))
            expected_backend = {
                "src-tauri/src/backend/providers.rs",
                "src-tauri/src/backend/commands.rs",
            }
            indexed_ids = {node["id"] for node in engine.sqlite.all_nodes()}
            if not expected_backend & indexed_ids:
                self.skipTest("current app graph.json is stale and lacks backend provider nodes")

            with patch.dict("os.environ", {"ORACLE_ASK_DISABLE_LLM": "1", "ORACLE_QUERY_EMBEDDER": "hash"}):
                answer = engine.ask("cloudflare worker secret rotation", limit=1)

                self.assertIn(answer["results"][0]["id"], expected_backend)

                container_answer = engine.ask("scaleway serverless container cpu", limit=1)
                self.assertEqual(
                    container_answer["results"][0]["id"],
                    "src-tauri/src/backend/providers.rs",
                )

    def _write_graph(self, root: Path) -> Path:
        graph = {
            "nodes": [
                {
                    "id": "workers/android/auth.js",
                    "label": "auth-worker",
                    "cluster": "Auth",
                    "community": "CF-Android",
                    "metadata": {
                        "docstring": "Validates JWT token and exposes POST /auth/refresh for Android clients.",
                        "dependencies": ["kv-session-store"],
                        "tags": ["Cloudflare Workers", "JWT"],
                    },
                },
                {
                    "id": "workers/browser/auth.js",
                    "label": "auth-worker",
                    "cluster": "Auth",
                    "community": "CF-Browser",
                    "metadata": {
                        "docstring": "Browser auth worker validates JWT and refreshes sessions.",
                        "dependencies": ["kv-session-store"],
                        "tags": ["Cloudflare Workers", "JWT"],
                    },
                },
                {
                    "id": "containers/scaleway/gpu-worker.py",
                    "label": "gpu-worker",
                    "cluster": "ML",
                    "community": "Scaleway",
                    "metadata": {
                        "docstring": "Runs GPU batch inference jobs on Scaleway.",
                        "dependencies": [],
                        "tags": ["Scaleway", "GPU"],
                    },
                },
                {
                    "id": "src-tauri/src/backend/providers.rs",
                    "label": "providers.rs",
                    "cluster": "Cloud",
                    "community": "Cloudflare",
                    "metadata": {
                        "docstring": "Cloudflare Workers inventory adapter and Worker secret rotation backend.",
                        "dependencies": [],
                        "tags": ["Cloudflare Workers", "secret rotation"],
                    },
                },
                {
                    "id": "src/types/config.ts",
                    "label": "config.ts",
                    "cluster": "Config",
                    "community": "Codebase",
                    "metadata": {
                        "docstring": "TypeScript config schema containing Provider, Secret, and ComputeConfig types.",
                        "dependencies": [],
                        "tags": ["config", "secret"],
                    },
                },
            ],
            "edges": [
                {
                    "source": "workers/android/auth.js",
                    "target": "kv-session-store",
                    "weight": 1.0,
                }
            ],
        }
        path = root / "graph.json"
        path.write_text(json.dumps(graph), encoding="utf-8")
        return path

    def _card(self, file_id: str) -> dict:
        return {
            "id": file_id,
            "label": Path(file_id).name,
            "area": "Codebase",
            "cluster_semantic": "Tests",
            "funzione_primaria": "test card",
            "espone_api": [],
            "dipende_da": [],
            "simile_a": [],
            "tecnologie": [],
            "file_sorgente": file_id,
            "ultima_modifica": "2026-05-29T00:00:00Z",
            "source": "oracle",
            "embedding_dims": 2,
        }


class OracleGitignoreSemanticsTest(unittest.TestCase):
    """The Oracle indexer must match the Rust/Polis gitignore semantics so the
    isometric map and the index see the SAME file set: gitignore negation
    (`!pattern`, last-match-wins), root anchoring (a leading `/` is root-relative,
    not a global name match), and honoring `.gitignore` itself.
    """

    def test_negation_rescues_explicitly_unignored_file(self):
        # gitignore last-match-wins: `build/` excludes the subtree but a later
        # `!build/keep.md` un-excludes that one file. Polis honors this; Oracle must
        # too, or a user's rescued path is kept on the map but lost from the index.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / ".oracleignore").write_text(
                "build/\n!build/keep.md\n",
                encoding="utf-8",
            )
            (root / "build").mkdir()
            (root / "build" / "x.js").write_text("export const x = 1;\n", encoding="utf-8")
            (root / "build" / "keep.md").write_text("# keep me\n", encoding="utf-8")
            (root / "src").mkdir()
            (root / "src" / "app.py").write_text("print('hi')\n", encoding="utf-8")

            files = {path.relative_to(root).as_posix() for path in collect_text_files(root)}

            self.assertIn("src/app.py", files)
            self.assertIn("build/keep.md", files)
            self.assertNotIn("build/x.js", files)

    def test_gitignore_is_honored_for_parity_with_polis(self):
        # The Rust scanner reads `.gitignore`; Oracle must too, or the map and the
        # index diverge. A `.gitignore` with `dist/` prunes `dist/x.js`.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / ".gitignore").write_text("dist/\n", encoding="utf-8")
            (root / "dist").mkdir()
            (root / "dist" / "x.js").write_text("export const x = 1;\n", encoding="utf-8")
            (root / "src").mkdir()
            (root / "src" / "app.py").write_text("print('hi')\n", encoding="utf-8")

            files = {path.relative_to(root).as_posix() for path in collect_text_files(root)}

            self.assertIn("src/app.py", files)
            self.assertNotIn("dist/x.js", files)

    def test_gitignore_negation_also_honored(self):
        # Negation semantics apply to `.gitignore` too, not only `.oracleignore`.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / ".gitignore").write_text("dist/\n!dist/keep.md\n", encoding="utf-8")
            (root / "dist").mkdir()
            (root / "dist" / "x.js").write_text("export const x = 1;\n", encoding="utf-8")
            (root / "dist" / "keep.md").write_text("# keep\n", encoding="utf-8")

            files = {path.relative_to(root).as_posix() for path in collect_text_files(root)}

            self.assertIn("dist/keep.md", files)
            self.assertNotIn("dist/x.js", files)

    def test_leading_slash_anchors_to_root_only(self):
        # An anchored `/legacy-out` matches only the root-level directory, NOT a
        # nested `src/legacy-out/`. The old `lstrip("/")` destroyed this, turning
        # the anchored pattern into a global name match. (A name NOT in the
        # built-in EXCLUDED_DIRS is used so this isolates the anchoring semantics.)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            (root / ".oracleignore").write_text("/legacy-out\n", encoding="utf-8")
            (root / "legacy-out").mkdir()
            (root / "legacy-out" / "old.py").write_text("print('old')\n", encoding="utf-8")
            (root / "src" / "legacy-out").mkdir(parents=True)
            (root / "src" / "legacy-out" / "live.py").write_text("print('live')\n", encoding="utf-8")

            files = {path.relative_to(root).as_posix() for path in collect_text_files(root)}

            self.assertNotIn("legacy-out/old.py", files)
            self.assertIn("src/legacy-out/live.py", files)

    def test_dir_is_install_root_no_io_predicate(self):
        # The no-IO predicate must recognize an install tree from os.walk's already
        # provided dirnames/filenames, with no extra scandir.
        self.assertTrue(
            dir_is_install_root(["numpy-1.26.4.dist-info", "numpy"], ["x.py"])
        )
        self.assertTrue(
            dir_is_install_root(["somepkg-2.0.egg-info"], [])
        )
        self.assertTrue(
            dir_is_install_root([], ["RECORD", "WHEEL", "top_level.txt"])
        )
        self.assertTrue(
            dir_is_install_root([], ["RECORD", "METADATA"])
        )
        # A real source dir has none of these markers.
        self.assertFalse(dir_is_install_root(["src", "tests"], ["app.py", "README.md"]))
        # RECORD alone (no WHEEL/METADATA) is not enough.
        self.assertFalse(dir_is_install_root([], ["RECORD"]))

    def test_collect_text_files_prunes_install_tree_via_no_io_predicate(self):
        # Parity with the existing vendored-env prune test, now driven by the no-IO
        # predicate. The walker must NOT call directory_contains_install_marker
        # (the per-directory second os.scandir is gone): if it did, the patched
        # version below would raise and fail the run.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            dist_info = root / "Orasis" / "numpy-1.26.4.dist-info"
            dist_info.mkdir(parents=True)
            (dist_info / "RECORD").write_text("numpy/__init__.py,,\n", encoding="utf-8")
            (dist_info / "METADATA").write_text("Name: numpy\n", encoding="utf-8")
            lib_pkg = root / "Orasis" / "numpy"
            lib_pkg.mkdir(parents=True)
            (lib_pkg / "core.py").write_text("def add(a, b):\n    return a + b\n", encoding="utf-8")
            real = root / "src"
            real.mkdir(parents=True)
            (real / "app.py").write_text("print('hi')\n", encoding="utf-8")

            def boom(*_args, **_kwargs):
                raise AssertionError(
                    "collect_text_files must not call directory_contains_install_marker "
                    "(double os.scandir per directory removed)"
                )

            with patch(
                "oracle.ingestion.chunk_index.directory_contains_install_marker", boom
            ):
                files = {path.relative_to(root).as_posix() for path in collect_text_files(root)}

            self.assertIn("src/app.py", files)
            self.assertNotIn("Orasis/numpy/core.py", files)
            self.assertNotIn("Orasis/numpy-1.26.4.dist-info/RECORD", files)


if __name__ == "__main__":
    unittest.main()
