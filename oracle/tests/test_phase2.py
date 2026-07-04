import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import oracle.config as oracle_config
from oracle.ingestion.classifier import is_placeholder_card
from oracle.ingestion.chunk_index import (
    MAX_INDEXED_FILES_LIMIT,
    build_chunks_for_file,
    chunk_index_status,
    chunk_path_allowed,
    collect_text_files,
    directory_contains_install_marker,
    file_signature,
    is_vendored_env_path,
    index_file_chunks,
    load_manifest,
    manifest_files_for_root,
    manifest_indexed_files,
    prune_excluded_chunks,
    save_manifest,
    strip_verbatim_prefix,
    sync_text_chunks,
)
from oracle.ingestion.embedder import (
    choose_device,
    embed_texts,
    sentence_transformer_kwargs,
)
from oracle.server.index_jobs import resolve_min_free_gb
from oracle.ingestion.learn import learn_files
from oracle.ingestion.parser import parse_file
from oracle.ingestion.retrieval_text import (
    SEMANTIC_PREFIX_PROFILE_VERSION,
    active_chunk_profile_version,
    chunk_embedding_text,
)
from oracle.server.mcp_handler import create_mcp_server, handle_tool_call
from oracle.server.answerer import (
    answer_from_context,
    answer_has_unsupported_grounding_terms,
    build_answer_prompt,
    redact_secret_tokens,
)
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore
from oracle.verify_runtime import model_names, runtime_status
from oracle.verify_coverage import coverage
from oracle.watcher.file_watcher import OracleWatcher
from oracle.watcher.trigger_learn import collect_watch_files


class OraclePhase2Test(unittest.TestCase):
    # Oracle answers are API-only now: answer_with_llm_config always calls
    # validate_remote_llm_config, which requires a remote provider + API key +
    # provider-matching HTTPS base URL. Tests that only want to exercise the
    # prompt/answer plumbing (and mock the network call) pass this valid
    # scaleway config explicitly via engine.ask(..., llm_config=API_LLM_CONFIG).
    # (Previously the local "ollama" path skipped validation entirely.)
    API_LLM_CONFIG = {
        "provider": "scaleway",
        "model": "voxtral-small-24b-2507",
        "base_url": "https://api.scaleway.ai/v1/chat/completions",
        "api_key": "sk-test-value",
    }

    def test_learn_files_upserts_oracle_source_cards_without_heavy_models(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "src" / "components" / "views" / "OracleView.tsx"
            source.parent.mkdir(parents=True)
            source.write_text(
                "import React from 'react';\n"
                "export function OracleView() { return <div>Architecture Oracle</div>; }\n",
                encoding="utf-8",
            )
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"

            learned = learn_files(
                [source],
                project_root=root,
                sqlite_path=sqlite_path,
                vector_path=vector_path,
                use_sentence_transformer=False,
                use_ollama_classifier=False,
            )

            self.assertEqual(learned, 1)
            card = SQLiteStore(sqlite_path).get_node("src/components/views/OracleView.tsx")
            self.assertIsNotNone(card)
            self.assertEqual(card["source"], "oracle")
            self.assertEqual(card["embedding_dims"], 1024)
            self.assertEqual(LanceStore(vector_path).count(), 1)

            report = coverage(sqlite_path)
            self.assertEqual(report["oracle_percent"], 100.0)
            self.assertEqual(report["oracle_percent"], 100.0)

            answer = QueryEngine(SQLiteStore(sqlite_path), LanceStore(vector_path)).ask(
                "where is the oracle page implemented",
                limit=1,
            )
            self.assertEqual(answer["results"][0]["id"], "src/components/views/OracleView.tsx")

    def test_parser_skips_sensitive_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # Secret DATA files (config/data extensions) must be skipped...
            secret = root / "src" / "secrets.yaml"
            secret.parent.mkdir(parents=True)
            secret.write_text("api_key: x\n", encoding="utf-8")

            self.assertIsNone(parse_file(secret, root))

            # ...but legitimate SOURCE code with the same words must be parsed.
            source = root / "src" / "tokens.ts"
            source.write_text("export const A = 1;\n", encoding="utf-8")
            secrets_view = root / "src" / "SecretsView.tsx"
            secrets_view.write_text("export function SecretsView(){ return null; }\n", encoding="utf-8")

            self.assertIsNotNone(parse_file(source, root))
            self.assertIsNotNone(parse_file(secrets_view, root))

    def test_redact_secret_tokens_scrubs_high_risk_strings(self):
        gh = "ghp_" + "a" * 36
        scw = "SCW" + "B" * 16
        hex_secret = "a1b2c3d4" * 6  # 48 hex chars
        text = (
            f"token = {gh}\n"
            f"export const SCALEWAY_KEY = '{scw}'\n"
            f"Authorization: Bearer abcDEF123456ghIJKL7890\n"
            f"api_key: sk-Test1234567890abcdEF\n"
            f"digest {hex_secret}\n"
        )
        redacted = redact_secret_tokens(text)
        self.assertNotIn(gh, redacted)
        self.assertNotIn(scw, redacted)
        self.assertNotIn(hex_secret, redacted)
        self.assertIn("[redacted-secret]", redacted)
        # Conservative: normal prose and short identifiers survive untouched.
        prose = "The cleanupScalewayInstance function stops a paid VM after a job."
        self.assertEqual(redact_secret_tokens(prose), prose)

    def test_build_answer_prompt_redacts_chunk_secrets(self):
        gh = "ghp_" + "z" * 36
        context = [
            {
                "ref": "C1",
                "chunk_id": "f.py#chunk-0",
                "file_source": "f.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 40,
                "text": f"live_token = {gh}",
            }
        ]
        prompt = build_answer_prompt("where is the token", context)
        self.assertNotIn(gh, prompt)
        self.assertIn("[redacted-secret]", prompt)

    def test_watcher_debounces_unique_supported_paths(self):
        batches = []
        watcher = OracleWatcher(batches.append, debounce_seconds=30)
        watcher.enqueue("src/App.tsx")
        watcher.enqueue("src/App.tsx")
        watcher.enqueue("dist/app.js")
        watcher.enqueue("README.txt")
        watcher.flush()

        self.assertEqual(batches, [["dist/app.js", "src/App.tsx"]])

    def test_trigger_learn_without_paths_collects_watch_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "src" / "App.tsx"
            ignored = root / "src" / "README.txt"
            source.parent.mkdir(parents=True)
            source.write_text("export const app = true;", encoding="utf-8")
            ignored.write_text("ignore", encoding="utf-8")

            self.assertEqual(collect_watch_files(root), [source])

    def test_mcp_handler_is_dispatchable_and_server_constructs(self):
        # create_mcp_server() needs the `mcp` package, which lives in the Oracle
        # venv (oracle-data/venv), not necessarily the bare test interpreter.
        # Skip when absent — same pattern as the lancedb-gated test.
        try:
            import mcp  # noqa: F401
        except ImportError:
            self.skipTest("mcp is not installed")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            graph = root / "graph.json"
            graph.write_text(
                json.dumps(
                    {
                        "nodes": [
                            {
                                "id": "oracle/server/mcp_handler.py",
                                "label": "mcp_handler.py",
                                "cluster": "Oracle",
                                "community": "Oracle",
                                "metadata": {"docstring": "Architecture Oracle MCP tools."},
                            }
                        ],
                        "edges": [],
                    }
                ),
                encoding="utf-8",
            )
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"
            from oracle.bootstrap.ingest_legacy_graph import ingest

            ingest(
                graph_path=graph,
                sqlite_path=sqlite_path,
                vector_path=vector_path,
                use_sentence_transformer=False,
            )
            old_sqlite = oracle_config.SQLITE_PATH
            old_vectors = oracle_config.LANCE_DB_PATH
            try:
                oracle_config.SQLITE_PATH = sqlite_path
                oracle_config.LANCE_DB_PATH = vector_path
                result = handle_tool_call("oracle_ask", {"query": "mcp tools", "limit": 1})
                self.assertEqual(result["results"][0]["id"], "oracle/server/mcp_handler.py")
                self.assertIsNotNone(create_mcp_server())
            finally:
                oracle_config.SQLITE_PATH = old_sqlite
                oracle_config.LANCE_DB_PATH = old_vectors

    def test_chunk_index_is_resumable_and_returns_semantic_context(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            worker = root / "cloudflare" / "workers" / "biovision-worker.ts"
            scaleway = root / "src-tauri" / "src" / "backend" / "providers.rs"
            session = root / "codex-sessions" / "log.md"
            test_result = root / "test-results" / ".last-run.json"
            vendor_bundle = root / "public" / "vendor" / "react.production.min.js"
            compiled_minified = root / "public" / "app.min.js"
            cmake_generated = root / "android" / "app" / ".cxx" / "Debug" / "arm64-v8a" / ".cmake" / "api" / "v1" / "reply" / "cache-v2.json"
            secrets_view = root / "src" / "components" / "SecretsView.tsx"
            worker.parent.mkdir(parents=True)
            scaleway.parent.mkdir(parents=True)
            session.parent.mkdir(parents=True)
            test_result.parent.mkdir(parents=True)
            vendor_bundle.parent.mkdir(parents=True)
            compiled_minified.parent.mkdir(parents=True, exist_ok=True)
            cmake_generated.parent.mkdir(parents=True)
            secrets_view.parent.mkdir(parents=True)
            worker.write_text(
                "export function limitGpuSpawning() {\n"
                "  return 'Biovision worker limits Scaleway GPU spawning by checking active training jobs';\n"
                "}\n",
                encoding="utf-8",
            )
            scaleway.write_text("pub fn spawn_gpu() {}\n", encoding="utf-8")
            session.write_text("do not index agent session noise", encoding="utf-8")
            test_result.write_text('{"status":"passed"}', encoding="utf-8")
            vendor_bundle.write_text("/* React minified vendor bundle */", encoding="utf-8")
            compiled_minified.write_text("function app(){return true}", encoding="utf-8")
            cmake_generated.write_text('{"generated":"cmake"}', encoding="utf-8")
            secrets_view.write_text("export function SecretsView() { return null; }\n", encoding="utf-8")

            collected = [path.relative_to(root).as_posix() for path in collect_text_files(root)]
            self.assertIn("src/components/SecretsView.tsx", collected)
            self.assertNotIn("codex-sessions/log.md", collected)
            self.assertNotIn("test-results/.last-run.json", collected)
            self.assertNotIn("public/vendor/react.production.min.js", collected)
            self.assertNotIn("public/app.min.js", collected)
            self.assertNotIn("android/app/.cxx/Debug/arm64-v8a/.cmake/api/v1/reply/cache-v2.json", collected)

            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            first = index_file_chunks(
                root,
                sqlite_path,
                chunk_vectors,
                manifest_path=manifest,
                batch_files=1,
                max_batches=1,
                min_free_gb=0,
                use_sentence_transformer=False,
            )
            self.assertEqual(first["status"], "paused_batch_limit")
            second = index_file_chunks(
                root,
                sqlite_path,
                chunk_vectors,
                manifest_path=manifest,
                batch_files=2,
                min_free_gb=0,
                use_sentence_transformer=False,
            )
            self.assertEqual(second["status"], "complete")
            status = chunk_index_status(root, sqlite_path, chunk_vectors, manifest)
            self.assertEqual(status["pending_files"], 0)
            self.assertEqual(status["sqlite_chunk_files"], 3)

            with patch.dict(os.environ, {"ORACLE_QUERY_EMBEDDER": "hash"}):
                engine = QueryEngine(SQLiteStore(sqlite_path), LanceStore(root / "vectors.json"), LanceStore(chunk_vectors))
                context = engine.context("how worker biovision limits gpu spawning in scaleway", limit=1)

            self.assertEqual(context[0]["file_source"], "cloudflare/workers/biovision-worker.ts")
            self.assertIn("limits Scaleway GPU spawning", context[0]["text"])

    def test_text_chunk_sync_makes_unembedded_files_retrievable(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "aspis-biovision" / "deploy" / "scaleway-vault" / "destroy.sh"
            source.parent.mkdir(parents=True)
            source.write_text(
                "#!/usr/bin/env bash\n"
                "# DESTROYS paid resources and user data.\n"
                "echo 'delete Scaleway Object Storage bucket and Serverless Postgres instance'\n",
                encoding="utf-8",
            )
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "empty-vectors.json"

            synced = sync_text_chunks(root, sqlite_path, batch_files=10)
            engine = QueryEngine(SQLiteStore(sqlite_path), LanceStore(vector_path), LanceStore(vector_path))
            context = engine.context("what does scaleway vault destroy do", limit=1)

            self.assertEqual(synced["files"], 1)
            self.assertEqual(context[0]["file_source"], "aspis-biovision/deploy/scaleway-vault/destroy.sh")
            self.assertEqual(context[0]["retrieval"], "lexical")
            self.assertIn("Serverless Postgres", context[0]["text"])

    def test_text_chunk_sync_is_incremental_and_skips_unchanged_files(self):
        # P4: on a completed, unchanged workspace the warm text sync must do
        # near-zero work — every file is skipped because the manifest already
        # records its current size/mtime/chunk_profile and sqlite holds its
        # chunks. This is what stops the "always indexing" residue.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "workspace"
            root.mkdir()
            source = root / "src" / "worker.ts"
            source.parent.mkdir(parents=True)
            source.write_text(
                "export function worker() { return 'Cloudflare Worker'; }\n",
                encoding="utf-8",
            )
            # Keep the sqlite + manifest OUTSIDE the indexed root so they are not
            # themselves collected as workspace text files (in production they live
            # under the excluded `oracle-data/`).
            store_dir = Path(tmp) / "store"
            store_dir.mkdir()
            sqlite_path = store_dir / "metadata.sqlite"
            manifest_path = store_dir / "chunk-index-manifest.json"

            # First sync: nothing in the manifest yet, so the file is chunked.
            first = sync_text_chunks(
                root, sqlite_path, batch_files=10, manifest_path=manifest_path
            )
            self.assertEqual(first["files"], 1)
            self.assertEqual(first["skipped"], 0)

            # Record the current signature in the manifest (mirrors what the dense
            # index pass writes after embedding) so the incremental check can match.
            manifest = load_manifest(manifest_path)
            manifest_files = manifest_files_for_root(manifest, root.resolve(), create=True)
            sqlite = SQLiteStore(sqlite_path)
            file_id = "src/worker.ts"
            chunk_count = len(sqlite.chunks_for_file(file_id))
            manifest_files[file_id] = file_signature(source, chunks=chunk_count)
            save_manifest(manifest_path, manifest)

            # Second sync, unchanged file: must SKIP it (no re-chunk).
            second = sync_text_chunks(
                root, sqlite_path, batch_files=10, manifest_path=manifest_path
            )
            self.assertEqual(second["files"], 0)
            self.assertEqual(second["skipped"], 1)

            # Touching the file (new mtime/size) makes it pending again.
            source.write_text(
                "export function worker() { return 'Cloudflare Worker v2 changed'; }\n",
                encoding="utf-8",
            )
            third = sync_text_chunks(
                root, sqlite_path, batch_files=10, manifest_path=manifest_path
            )
            self.assertEqual(third["files"], 1)
            self.assertEqual(third["skipped"], 0)

            # force=True rewrites everything regardless of the manifest.
            forced = sync_text_chunks(
                root,
                sqlite_path,
                batch_files=10,
                manifest_path=manifest_path,
                force=True,
            )
            self.assertEqual(forced["files"], 1)
            self.assertEqual(forced["skipped"], 0)

    def test_manifest_collapses_windows_verbatim_prefix_to_one_root(self):
        # P4: the `\\?\C:\…` (verbatim) and plain `C:\…` forms of the SAME
        # workspace must map to ONE manifest key, and a stale verbatim duplicate
        # must be merged + pruned — otherwise Python treats the verbatim form as a
        # new workspace and re-embeds every file ("always indexing").
        self.assertEqual(strip_verbatim_prefix(r"\\?\C:\Users\me\aspis bio"), r"C:\Users\me\aspis bio")
        self.assertEqual(strip_verbatim_prefix(r"\\?\UNC\server\share\x"), r"\\server\share\x")
        self.assertEqual(strip_verbatim_prefix(r"C:\Users\me\aspis bio"), r"C:\Users\me\aspis bio")

        plain = r"C:\Users\me\aspis bio"
        verbatim = r"\\?\C:\Users\me\aspis bio"
        manifest = {
            "version": 2,
            "roots": {
                plain: {"files": {"a.py": {"size": 1, "mtime_ns": 1, "chunks": 1}}},
                verbatim: {"files": {"b.py": {"size": 2, "mtime_ns": 2, "chunks": 1}}},
            },
        }
        # Addressing the workspace by its plain form must fold the verbatim entry
        # in and drop it, leaving a single root with BOTH files.
        files = manifest_files_for_root(manifest, Path(plain), create=True)
        self.assertEqual(set(manifest["roots"].keys()), {plain})
        self.assertEqual(set(files.keys()), {"a.py", "b.py"})

    def test_sqlite_store_uses_wal_and_busy_timeout(self):
        # P3: the metadata store must run in WAL with an explicit busy timeout so a
        # reader (/health, /runtime, /ask) is not blocked/"database is locked"
        # behind an index write transaction.
        with tempfile.TemporaryDirectory() as tmp:
            sqlite_path = Path(tmp) / "metadata.sqlite"
            store = SQLiteStore(sqlite_path)
            with store._connect() as conn:
                journal_mode = conn.execute("PRAGMA journal_mode").fetchone()[0]
                busy_timeout = conn.execute("PRAGMA busy_timeout").fetchone()[0]
            self.assertEqual(str(journal_mode).lower(), "wal")
            self.assertEqual(busy_timeout, SQLiteStore._BUSY_TIMEOUT_MS)

    def test_docs_use_larger_qwen_embedding_chunks_than_code(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            doc = root / "docs" / "architecture.md"
            code = root / "src" / "worker.ts"
            doc.parent.mkdir(parents=True)
            code.parent.mkdir(parents=True)
            doc.write_text(("Architecture paragraph about Scaleway and Oracle.\n" * 520), encoding="utf-8")
            code.write_text(("export function workerStep() { return 'small focused code chunk'; }\n" * 520), encoding="utf-8")

            doc_chunks = build_chunks_for_file(doc, root)
            code_chunks = build_chunks_for_file(code, root)

            self.assertGreater(max(len(chunk["text"]) for chunk in doc_chunks), 8000)
            self.assertLessEqual(max(len(chunk["text"]) for chunk in doc_chunks), 12000)
            self.assertLessEqual(max(len(chunk["text"]) for chunk in code_chunks), 5000)
            self.assertGreater(len(code_chunks), len(doc_chunks))

    def test_semantic_prefix_embedding_text_adds_domains_symbols_and_raw_chunk(self):
        chunk = {
            "id": "src-tauri/src/backend/commands.rs#chunk-0000",
            "file_id": "src-tauri/src/backend/commands.rs",
            "text": (
                "pub fn rotate_cloudflare_worker_secret() {}\n"
                "fn put_cloudflare_worker_secret() {}\n"
            ),
        }

        semantic_text = chunk_embedding_text(chunk, profile="semantic-prefix-v2")
        raw_text = chunk_embedding_text(chunk, profile="raw")

        self.assertIn("TASK: retrieve Aspis Bio", semantic_text)
        self.assertIn("SOURCE_PATH: src-tauri/src/backend/commands.rs", semantic_text)
        self.assertIn("SOURCE_KIND: implementation_primary", semantic_text)
        self.assertIn("DOMAIN_TAGS: cloudflare_worker_secret_rotation", semantic_text)
        self.assertIn("SYMBOLS: rotate_cloudflare_worker_secret", semantic_text)
        self.assertIn("Where is Cloudflare Worker secret rotation implemented?", semantic_text)
        self.assertIn("RAW_CHUNK:", semantic_text)
        self.assertIn("pub fn rotate_cloudflare_worker_secret", semantic_text)
        self.assertEqual(raw_text, f"{chunk['file_id']}\n{chunk['text']}")

    def test_chunk_manifest_profile_tracks_embedding_profile(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "src" / "worker.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export function worker() { return 'Cloudflare Worker'; }\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            with patch.dict(os.environ, {"ORACLE_EMBED_PROFILE": "raw"}):
                result = index_file_chunks(
                    root,
                    sqlite_path,
                    chunk_vectors,
                    manifest_path=manifest,
                    min_free_gb=0,
                    use_sentence_transformer=False,
                )
                status = chunk_index_status(root, sqlite_path, chunk_vectors, manifest)
            self.assertEqual(result["status"], "complete")
            self.assertEqual(status["stale_files"], 0)

            with patch.dict(os.environ, {"ORACLE_EMBED_PROFILE": "semantic-prefix-v2"}):
                status = chunk_index_status(root, sqlite_path, chunk_vectors, manifest)

            self.assertEqual(status["chunk_profile"], SEMANTIC_PREFIX_PROFILE_VERSION)
            self.assertEqual(status["stale_files"], 1)

    def test_active_chunk_profile_version_is_current_semantic_prefix(self):
        # Change 3: the active semantic-prefix profile version must be the bumped
        # constant so that the c2500 re-chunk is forced corpus-wide.
        with patch.dict(os.environ, {"ORACLE_EMBED_PROFILE": "semantic-prefix-v2"}):
            self.assertEqual(active_chunk_profile_version(), SEMANTIC_PREFIX_PROFILE_VERSION)
        self.assertEqual(SEMANTIC_PREFIX_PROFILE_VERSION, "semantic-prefix-qwen3-2026-06-02-c2500")

    def test_old_chunk_profile_manifest_entry_is_stale(self):
        # Change 3: a manifest entry recorded under the OLD profile string must be
        # treated as stale (needs re-index) even when size+mtime are unchanged, so
        # the profile bump re-chunks every existing file at the new 2500 size.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            source = root / "src" / "worker.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export function worker() { return 'Cloudflare Worker'; }\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            with patch.dict(os.environ, {"ORACLE_EMBED_PROFILE": "semantic-prefix-v2"}):
                index_file_chunks(
                    root,
                    sqlite_path,
                    chunk_vectors,
                    manifest_path=manifest,
                    min_free_gb=0,
                    use_sentence_transformer=False,
                )
                fresh = chunk_index_status(root, sqlite_path, chunk_vectors, manifest)
                self.assertEqual(fresh["stale_files"], 0)

                # Rewrite the manifest entry to the OLD profile string, leaving
                # size/mtime intact, and confirm it is now considered stale.
                data = load_manifest(manifest)
                files = manifest_files_for_root(data, root, create=False)
                self.assertTrue(files)
                for entry in files.values():
                    entry["chunk_profile"] = "semantic-prefix-qwen3-2026-05-28"
                save_manifest(manifest, data)

                stale = chunk_index_status(root, sqlite_path, chunk_vectors, manifest)
            self.assertEqual(stale["stale_files"], 1)

    def test_grounding_term_from_uncited_retrieved_chunk_is_accepted(self):
        # Change 1: an identifier present in a RETRIEVED-but-UNCITED chunk is
        # grounded (we showed it to the model) and must NOT flag the answer.
        context = [
            {"ref": "1", "file_source": "a.py", "chunk_id": "a#1", "text": "def cited_helper(): pass"},
            {"ref": "2", "file_source": "b.py", "chunk_id": "b#1", "text": "def uncited_helper(): pass"},
        ]
        citations = [{"ref": "1"}]
        answer = "It calls `uncited_helper` which is defined in `b.py`."
        self.assertFalse(answer_has_unsupported_grounding_terms(answer, citations, context))

    def test_grounding_tolerates_up_to_two_stray_terms(self):
        # Change 1: up to 2 unsupported terms are tolerated.
        context = [
            {"ref": "1", "file_source": "a.py", "chunk_id": "a#1", "text": "def real_helper(): pass"},
        ]
        citations = [{"ref": "1"}]
        answer = "It uses `real_helper`, `strayOne` and `strayTwo`."
        self.assertFalse(answer_has_unsupported_grounding_terms(answer, citations, context))

    def test_grounding_flags_many_fabricated_identifiers(self):
        # Change 1: a fabricated answer with >2 invented identifiers is flagged.
        context = [
            {"ref": "1", "file_source": "a.py", "chunk_id": "a#1", "text": "def real_helper(): pass"},
        ]
        citations = [{"ref": "1"}]
        answer = "It calls `fakeOne`, `fakeTwo`, `fakeThree` and `fakeFour` in `bogus/path.py`."
        self.assertTrue(answer_has_unsupported_grounding_terms(answer, citations, context))

    def test_chunk_index_batches_embedding_across_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for index in range(3):
                source = root / "src" / f"worker_{index}.ts"
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text(
                    f"export const worker{index} = 'Scaleway dense indexing batch';\n",
                    encoding="utf-8",
                )
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"
            embed_calls = []

            def fake_embed(texts, **_kwargs):
                embed_calls.append(list(texts))
                return [[1.0, 0.0] for _text in texts]

            with patch("oracle.ingestion.chunk_index.embed_texts", side_effect=fake_embed):
                result = index_file_chunks(
                    root,
                    sqlite_path,
                    chunk_vectors,
                    manifest_path=manifest,
                    batch_files=3,
                    batch_chunks=100,
                    min_free_gb=0,
                    use_sentence_transformer=True,
                    require_sentence_transformer=True,
                )

            self.assertEqual(result["status"], "complete")
            self.assertEqual(len(embed_calls), 1)
            self.assertEqual(len(embed_calls[0]), 3)
            self.assertEqual(LanceStore(chunk_vectors).count(), 3)
            status = chunk_index_status(root, sqlite_path, chunk_vectors, manifest)
            self.assertEqual(status["pending_files"], 0)

    def test_cuda_embedding_loader_uses_half_precision(self):
        kwargs = sentence_transformer_kwargs(allow_download=False, device="cuda")

        self.assertEqual(str(kwargs["model_kwargs"]["torch_dtype"]), "torch.float16")

    def test_empty_allowed_file_does_not_remain_stale_after_index(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            empty_doc = root / "aspis-handoff.md"
            empty_doc.write_text("", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            result = index_file_chunks(
                root,
                sqlite_path,
                chunk_vectors,
                manifest_path=manifest,
                min_free_gb=0,
                use_sentence_transformer=False,
            )
            status = chunk_index_status(root, sqlite_path, chunk_vectors, manifest)

            self.assertEqual(result["status"], "complete")
            self.assertEqual(status["pending_files"], 0)
            self.assertEqual(status["stale_files"], 0)

    def test_context_ranking_prefers_answer_chunk_over_dependency_mentions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "empty-vectors.json"
            sqlite = SQLiteStore(sqlite_path)
            chunks = [
                {
                    "id": "aspis-lab/docs/WORKER_DEPENDENCIES.md#chunk-0000",
                    "file_id": "aspis-lab/docs/WORKER_DEPENDENCIES.md",
                    "chunk_index": 0,
                    "start_char": 0,
                    "end_char": 200,
                    "text": "Open questions mention worker biovision limit GPU spawning in Scaleway, but this section does not answer the mechanism.",
                    "file_sorgente": "aspis-lab/docs/WORKER_DEPENDENCIES.md",
                    "ultima_modifica": "2026-05-28T00:00:00Z",
                    "embedding_dims": 1024,
                },
                {
                    "id": "aspis-biovision/docs/scaleway-services.md#chunk-0000",
                    "file_id": "aspis-biovision/docs/scaleway-services.md",
                    "chunk_index": 0,
                    "start_char": 0,
                    "end_char": 260,
                    "text": "Cloudflare Worker keeps Biovision on Scaleway Serverless Containers with min_scale=0 and scale-to-zero. No GPU is required at this scale; CPU specialists handle bursts and max_scale controls parallelism.",
                    "file_sorgente": "aspis-biovision/docs/scaleway-services.md",
                    "ultima_modifica": "2026-05-28T00:00:00Z",
                    "embedding_dims": 1024,
                },
            ]
            sqlite.replace_chunks_for_files([chunk["file_id"] for chunk in chunks], chunks)
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))

            context = engine.context("how the worker biovision limit the gpu spawning in scaleway", limit=1)

            self.assertEqual(context[0]["file_source"], "aspis-biovision/docs/scaleway-services.md")

    def test_ask_generates_local_qwen_answer_with_chunk_citations(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite = SQLiteStore(root / "metadata.sqlite")
            vector_path = root / "empty-vectors.json"
            chunk = {
                "id": "aspis-biovision/docs/scaleway-services.md#chunk-0000",
                "file_id": "aspis-biovision/docs/scaleway-services.md",
                "chunk_index": 0,
                "start_char": 120,
                "end_char": 360,
                "text": "Biovision runs on Scaleway Serverless Containers with min_scale=0. No GPU is required for this path; max_scale controls parallel CPU bursts.",
                "file_sorgente": "aspis-biovision/docs/scaleway-services.md",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([chunk["file_id"]], [chunk])
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))
            captured = {}

            def fake_generate(prompt, *args, **_kwargs):
                captured["prompt"] = prompt
                return json.dumps(
                    {
                        "answer": "Biovision evita GPU qui usando Serverless Containers con min_scale=0; il parallelismo CPU e' limitato da max_scale.",
                        "citations": [{"ref": "C1"}],
                        "not_found": False,
                        "suggested_path": None,
                    }
                )

            with patch("oracle.server.answerer.generate_with_openai_compatible", side_effect=fake_generate):
                answer = engine.ask("how does biovision limit scaleway gpu spawning", limit=1, llm_config=self.API_LLM_CONFIG)

            self.assertIn("min_scale=0", answer["answer"])
            self.assertFalse(answer["not_found"])
            self.assertEqual(answer["citations"][0]["file_source"], chunk["file_sorgente"])
            self.assertEqual(answer["citations"][0]["chunk_id"], chunk["id"])
            self.assertEqual(answer["citations"][0]["start_char"], 120)
            self.assertIn(chunk["text"], captured["prompt"])
            self.assertIn("Always answer in English", captured["prompt"])

    def test_ask_falls_back_to_extractive_answer_when_local_llm_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite = SQLiteStore(root / "metadata.sqlite")
            vector_path = root / "empty-vectors.json"
            chunk = {
                "id": "src-tauri/src/backend/providers.rs#chunk-0000",
                "file_id": "src-tauri/src/backend/providers.rs",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 180,
                "text": "Scaleway VM lifecycle actions are handled by perform_scaleway_resource_action_request for start, stop, terminate, and delete.",
                "file_sorgente": "src-tauri/src/backend/providers.rs",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([chunk["file_id"]], [chunk])
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))

            with patch(
                "oracle.server.answerer.generate_with_openai_compatible",
                side_effect=RuntimeError("model requires more system memory"),
            ):
                answer = engine.ask("which files control Scaleway VM lifecycle actions", limit=1, llm_config=self.API_LLM_CONFIG)

            self.assertFalse(answer["not_found"])
            self.assertIn("Oracle found relevant code evidence", answer["answer"])
            self.assertIn("Scaleway VM lifecycle actions", answer["answer"])
            self.assertEqual(answer["citations"][0]["file_source"], chunk["file_sorgente"])

    def test_ask_falls_back_to_extractive_answer_when_llm_returns_empty_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite = SQLiteStore(root / "metadata.sqlite")
            vector_path = root / "empty-vectors.json"
            chunk = {
                "id": "src/components/dashboard/WorkersTable.tsx#chunk-0000",
                "file_id": "src/components/dashboard/WorkersTable.tsx",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 160,
                "text": "WorkersTable calls askOracle for Cloudflare worker details and displays the returned citations.",
                "file_sorgente": "src/components/dashboard/WorkersTable.tsx",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([chunk["file_id"]], [chunk])
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))

            with patch("oracle.server.answerer.generate_with_openai_compatible", return_value=""):
                answer = engine.ask("how does WorkersTable ask Oracle", limit=1, llm_config=self.API_LLM_CONFIG)

            self.assertFalse(answer["not_found"])
            self.assertIn("LLM returned empty or invalid JSON", answer["answer"])
            self.assertEqual(answer["citations"][0]["chunk_id"], chunk["id"])

    def test_domain_extractive_answer_handles_scaleway_paid_cleanup(self):
        chunks = [
            {
                "chunk_id": "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs#chunk-0000",
                "file_source": "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 500,
                "retrieval": "lexical",
                "score": 50.0,
                "text": (
                    "export async function cleanupScalewayInstanceAfterTerminal() { "
                    "await terminateScalewayInstance(); await releaseScalewayInstanceSlot(); "
                    "delete instance with with_volumes=all; }"
                ),
            }
        ]

        with patch("oracle.server.answerer.generate_with_openai_compatible", return_value=""):
            answer = answer_from_context(
                "Where do we stop paid Scaleway compute resources after a job or terminal session is done?",
                chunks,
            )

        self.assertEqual(answer["answer_source"], "extractive_synthesis")
        self.assertIn("cleanupScalewayInstanceAfterTerminal", answer["answer"])
        self.assertIn("terminateScalewayInstance", answer["answer"])
        self.assertIn("releaseScalewayInstanceSlot", answer["answer"])
        self.assertFalse(answer["not_found"])

    def test_domain_extractive_answer_handles_agent_project_workflow(self):
        chunks = [
            {
                "chunk_id": "oracle/server/aspis_mcp.py#chunk-0000",
                "file_source": "oracle/server/aspis_mcp.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 500,
                "retrieval": "dense+lexical",
                "score": 50.0,
                "text": (
                    "MCP tools include oracle_ask, oracle_context, project_list, project_get, "
                    "project_claim_task, and project_update_status for terminal agents."
                ),
            }
        ]

        with patch("oracle.server.answerer.generate_with_openai_compatible", return_value=""):
            answer = answer_from_context(
                "How do terminal agents know the current project task and mark it finished without editing the UI manually?",
                chunks,
            )

        self.assertEqual(answer["answer_source"], "extractive_synthesis")
        self.assertIn("project_claim_task", answer["answer"])
        self.assertIn("project_update_status", answer["answer"])
        self.assertIn("oracle_ask", answer["answer"])
        self.assertFalse(answer["not_found"])

    def test_domain_extractive_answer_handles_oracle_privacy_gate(self):
        chunks = [
            {
                "chunk_id": "src-tauri/src/backend/vault.rs#chunk-0000",
                "file_source": "src-tauri/src/backend/vault.rs",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 500,
                "retrieval": "lexical",
                "score": 50.0,
                "text": "Oracle settings allow only ollama, scaleway, infomaniak, and mistral providers.",
            },
            {
                "chunk_id": "oracle/server/answerer.py#chunk-0000",
                "file_source": "oracle/server/answerer.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 500,
                "retrieval": "lexical",
                "score": 49.0,
                "text": (
                    "Remote Oracle LLM provider is not allowlisted unless provider is scaleway, infomaniak, or mistral; "
                    "the provider allowlist is the only privacy gate."
                ),
            },
        ]

        with patch("oracle.server.answerer.generate_with_openai_compatible", return_value=""):
            answer = answer_from_context(
                "Where is the rule that only privacy safe AI providers can be used for Oracle answers?",
                chunks,
            )

        self.assertEqual(answer["answer_source"], "extractive_synthesis")
        self.assertIn("scaleway", answer["answer"].lower())
        self.assertIn("infomaniak", answer["answer"].lower())
        self.assertIn("mistral", answer["answer"].lower())
        self.assertIn("allowlist", answer["answer"].lower())
        self.assertFalse(answer["not_found"])

    def test_ask_rejects_non_english_llm_answer_even_with_valid_citation(self):
        chunks = [
            {
                "chunk_id": "src-tauri/src/backend/providers.rs#chunk-0000",
                "file_source": "src-tauri/src/backend/providers.rs",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 220,
                "retrieval": "lexical",
                "score": 50.0,
                "text": "Scaleway VM lifecycle actions are handled by perform_scaleway_resource_action_request for start, stop, terminate, and delete.",
            }
        ]

        with patch(
            "oracle.server.answerer.generate_with_openai_compatible",
            return_value=json.dumps(
                {
                    "answer": "La risposta e' che il codice usa perform_scaleway_resource_action_request.",
                    "citations": [{"ref": "C1"}],
                    "not_found": False,
                    "suggested_path": None,
                }
            ),
        ):
            answer = answer_from_context("which file controls Scaleway VM lifecycle actions", chunks, llm_config=self.API_LLM_CONFIG)

        self.assertEqual(answer["answer_source"], "extractive_fallback")
        self.assertEqual(answer["fallback_reason"], "LLM returned a non-English answer")
        self.assertNotIn("La risposta", answer["answer"])
        self.assertIn("perform_scaleway_resource_action_request", answer["answer"])

    def test_ask_rejects_unsupported_identifiers_even_with_valid_citation(self):
        chunks = [
            {
                "chunk_id": "oracle/server/aspis_mcp.py#chunk-0000",
                "file_source": "oracle/server/aspis_mcp.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 240,
                "retrieval": "lexical",
                "score": 50.0,
                "text": "Agents call project_claim_task and project_update_status to update task status through MCP.",
            }
        ]

        # The grounding guard now tolerates up to 2 stray terms (a real identifier
        # from a retrieved-but-uncited chunk should not nuke a good answer), so a
        # hallucinated answer must invent MORE than 2 identifiers/paths to be
        # rejected by this guard — which is exactly what a fabricated answer does.
        with patch(
            "oracle.server.answerer.generate_with_openai_compatible",
            return_value=json.dumps(
                {
                    "answer": (
                        "Agents call `project_claim_task`, then invented helpers "
                        "`teleportTaskToDone`, `warpStatusForward`, `beamTaskComplete` "
                        "in `oracle/server/madeup_path.py`."
                    ),
                    "citations": [{"ref": "C1"}],
                    "not_found": False,
                    "suggested_path": None,
                }
            ),
        ):
            answer = answer_from_context("how do agents update project status", chunks, llm_config=self.API_LLM_CONFIG)

        self.assertNotEqual(answer["answer_source"], "llm")
        self.assertEqual(answer["fallback_reason"], "LLM answer included unsupported identifiers or paths")
        self.assertNotIn("teleportTaskToDone", answer["answer"])

    def test_ask_rejects_unsupported_natural_language_claims(self):
        chunks = [
            {
                "chunk_id": "oracle/server/aspis_mcp.py#chunk-0000",
                "file_source": "oracle/server/aspis_mcp.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 260,
                "retrieval": "lexical",
                "score": 50.0,
                "text": "Verifier agents can set done only after review, with evidence and confidence >= 0.70.",
            }
        ]

        with patch(
            "oracle.server.answerer.generate_with_openai_compatible",
            return_value=json.dumps(
                {
                    "answer": "The system automatically marks tasks done without verifier review.",
                    "citations": [{"ref": "C1"}],
                    "not_found": False,
                    "suggested_path": None,
                }
            ),
        ):
            answer = answer_from_context("how are project tasks closed", chunks, llm_config=self.API_LLM_CONFIG)

        self.assertNotEqual(answer["answer_source"], "llm")
        self.assertEqual(answer["fallback_reason"], "LLM answer included unsupported natural-language claims")
        self.assertNotIn("automatically marks tasks done", answer["answer"])

    def test_ask_rejects_spanish_llm_answer_even_with_valid_citation(self):
        chunks = [
            {
                "chunk_id": "oracle/server/aspis_mcp.py#chunk-0000",
                "file_source": "oracle/server/aspis_mcp.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 240,
                "retrieval": "lexical",
                "score": 50.0,
                "text": "Agents call project_claim_task and project_update_status to update task status through MCP.",
            }
        ]

        with patch(
            "oracle.server.answerer.generate_with_openai_compatible",
            return_value=json.dumps(
                {
                    "answer": "Los agentes usan project_update_status para cambiar el estado de la tarea.",
                    "citations": [{"ref": "C1"}],
                    "not_found": False,
                    "suggested_path": None,
                }
            ),
        ):
            answer = answer_from_context("how do agents update project status", chunks, llm_config=self.API_LLM_CONFIG)

        self.assertNotEqual(answer["answer_source"], "llm")
        self.assertEqual(answer["fallback_reason"], "LLM returned a non-English answer")

    def test_not_found_ignores_llm_suggested_path(self):
        chunks = [
            {
                "chunk_id": "oracle/server/answerer.py#chunk-0000",
                "file_source": "oracle/server/answerer.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 120,
                "retrieval": "lexical",
                "score": 10.0,
                "text": "Oracle validates grounded answer JSON and citations.",
            }
        ]

        with patch(
            "oracle.server.answerer.generate_with_openai_compatible",
            return_value=json.dumps(
                {
                    "answer": "not found in corpus: no matching implementation.",
                    "citations": [],
                    "not_found": True,
                    "suggested_path": "C:\\\\made-up\\\\secret-place",
                }
            ),
        ):
            answer = answer_from_context("where is the moon billing terraform pipeline", chunks, llm_config=self.API_LLM_CONFIG)

        self.assertTrue(answer["not_found"])
        self.assertEqual(answer["suggested_path"], "oracle/server/answerer.py")

    def test_ask_disable_llm_returns_bounded_extractive_answer(self):
        chunks = [
            {
                "chunk_id": "oracle/server/aspis_mcp.py#chunk-0000",
                "file_source": "oracle/server/aspis_mcp.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 220,
                "retrieval": "lexical",
                "score": 50.0,
                "text": "Agents call project_claim_task and project_update_status to update task status through MCP.",
            }
        ]

        with patch("oracle.server.answerer.generate_with_openai_compatible") as generate:
            with patch.dict(os.environ, {"ORACLE_ASK_DISABLE_LLM": "1"}):
                answer = answer_from_context("how do agents update project status", chunks)

        generate.assert_not_called()
        self.assertFalse(answer["not_found"])
        self.assertEqual(answer["answer_source"], "extractive_synthesis")
        self.assertIn("project_update_status", answer["answer"])

    def test_ask_accepts_grounded_identifiers_from_cited_context(self):
        chunks = [
            {
                "chunk_id": "oracle/server/aspis_mcp.py#chunk-0000",
                "file_source": "oracle/server/aspis_mcp.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 240,
                "retrieval": "lexical",
                "score": 50.0,
                "text": "Agents call project_claim_task and project_update_status to update task status through MCP.",
            }
        ]

        with patch(
            "oracle.server.answerer.generate_with_openai_compatible",
            return_value=json.dumps(
                {
                    "answer": "Agents use `project_claim_task` first, then write the handoff with `project_update_status`.",
                    "citations": [{"ref": "C1"}],
                    "not_found": False,
                    "suggested_path": None,
                }
            ),
        ):
            answer = answer_from_context("how do agents update project status", chunks, llm_config=self.API_LLM_CONFIG)

        self.assertEqual(answer["answer_source"], "llm")
        self.assertIn("project_update_status", answer["answer"])

    def test_ask_without_context_returns_not_found_without_qwen_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            engine = QueryEngine(
                SQLiteStore(root / "metadata.sqlite"),
                LanceStore(root / "empty-vectors.json"),
                LanceStore(root / "empty-chunks.json"),
            )

            with patch("oracle.server.answerer.generate_with_openai_compatible") as generate:
                answer = engine.ask("where is the moon billing terraform pipeline", limit=3)

            self.assertFalse(generate.called)
            self.assertTrue(answer["not_found"])
            self.assertIn("not found in corpus", answer["answer"])
            self.assertEqual(answer["citations"], [])
            self.assertIsNone(answer["suggested_path"])

    def test_mcp_oracle_ask_returns_generated_answer_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            vector_path = root / "vectors.json"
            chunk_vector_path = root / "chunks.json"
            sqlite = SQLiteStore(sqlite_path)
            chunk = {
                "id": "oracle/server/routes.py#chunk-0000",
                "file_id": "oracle/server/routes.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 180,
                "text": "The Oracle routes expose /ask and /context for local agents.",
                "file_sorgente": "oracle/server/routes.py",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([chunk["file_id"]], [chunk])
            old_sqlite = oracle_config.SQLITE_PATH
            old_vectors = oracle_config.LANCE_DB_PATH
            old_chunks = oracle_config.CHUNK_DB_PATH
            try:
                oracle_config.SQLITE_PATH = sqlite_path
                oracle_config.LANCE_DB_PATH = vector_path
                oracle_config.CHUNK_DB_PATH = chunk_vector_path
                with patch(
                    "oracle.server.answerer.generate_with_openai_compatible",
                    return_value=json.dumps(
                        {
                            "answer": "Gli agenti locali usano /ask per risposte e /context per recuperare chunk.",
                            "citations": [{"ref": "C1"}],
                            "not_found": False,
                            "suggested_path": None,
                        }
                    ),
                ):
                    result = handle_tool_call("oracle_ask", {"query": "how do local agents query oracle", "limit": 1})

                self.assertIn("/ask", result["answer"])
                self.assertEqual(result["citations"][0]["chunk_id"], chunk["id"])
                self.assertEqual(result["citations"][0]["chunk_index"], 0)
            finally:
                oracle_config.SQLITE_PATH = old_sqlite
                oracle_config.LANCE_DB_PATH = old_vectors
                oracle_config.CHUNK_DB_PATH = old_chunks

    def test_ask_prompt_uses_query_focused_excerpt_from_large_chunk(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite = SQLiteStore(root / "metadata.sqlite")
            vector_path = root / "empty-vectors.json"
            relevant = "The GPU spawning path is limited by min_scale=0 and max_scale controls CPU bursts."
            chunk_text = ("unrelated architecture notes.\n" * 360) + relevant + ("\nmore unrelated notes." * 360)
            chunk = {
                "id": "aspis-biovision/docs/large-scaleway.md#chunk-0000",
                "file_id": "aspis-biovision/docs/large-scaleway.md",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": len(chunk_text),
                "text": chunk_text,
                "file_sorgente": "aspis-biovision/docs/large-scaleway.md",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([chunk["file_id"]], [chunk])
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))
            captured = {}

            def fake_generate(prompt, *args, **_kwargs):
                captured["prompt"] = prompt
                return json.dumps(
                    {
                        "answer": "GPU spawning e' limitato da min_scale=0 e max_scale.",
                        "citations": [{"ref": "C1"}],
                        "not_found": False,
                        "suggested_path": None,
                    }
                )

            with patch("oracle.server.answerer.generate_with_openai_compatible", side_effect=fake_generate):
                answer = engine.ask("how is scaleway gpu spawning limited by min_scale and max_scale", limit=1, llm_config=self.API_LLM_CONFIG)

            self.assertFalse(answer["not_found"])
            self.assertIn(relevant, captured["prompt"])
            self.assertLess(len(captured["prompt"]), 6000)

    def test_ask_prompt_excludes_superseded_chunks_when_current_context_exists(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite = SQLiteStore(root / "metadata.sqlite")
            vector_path = root / "empty-vectors.json"
            current = {
                "id": "aspis-biovision/docs/scaleway-services.md#chunk-0000",
                "file_id": "aspis-biovision/docs/scaleway-services.md",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 220,
                "text": "Current Scaleway setup: Biovision uses Serverless Containers with min_scale=0 and no GPU required.",
                "file_sorgente": "aspis-biovision/docs/scaleway-services.md",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            superseded = {
                "id": "aspis-biovision/docs/adr/ADR-003-deployment.md#chunk-0000",
                "file_id": "aspis-biovision/docs/adr/ADR-003-deployment.md",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 300,
                "text": "Superseded by the Scaleway pivot. Historical architecture used Nebius for the VLM judge.",
                "file_sorgente": "aspis-biovision/docs/adr/ADR-003-deployment.md",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([current["file_id"], superseded["file_id"]], [current, superseded])
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))
            captured = {}

            def fake_generate(prompt, *args, **_kwargs):
                captured["prompt"] = prompt
                return json.dumps(
                    {
                        "answer": "Biovision usa Serverless Containers con min_scale=0 e senza GPU.",
                        "citations": [{"ref": "C1"}],
                        "not_found": False,
                        "suggested_path": None,
                    }
                )

            with patch("oracle.server.answerer.generate_with_openai_compatible", side_effect=fake_generate):
                answer = engine.ask("how does biovision limit gpu on scaleway", limit=2, llm_config=self.API_LLM_CONFIG)

            self.assertFalse(answer["not_found"])
            self.assertIn("Serverless Containers", captured["prompt"])
            self.assertNotIn("Nebius", captured["prompt"])

    def test_ask_prompt_keeps_biovision_query_out_of_orasis_context_when_possible(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite = SQLiteStore(root / "metadata.sqlite")
            vector_path = root / "empty-vectors.json"
            biovision = {
                "id": "aspis-biovision/docs/scaleway-services.md#chunk-0000",
                "file_id": "aspis-biovision/docs/scaleway-services.md",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 240,
                "text": "Biovision uses Scaleway Serverless Containers with min_scale=0, max_scale=10, and no GPU required.",
                "file_sorgente": "aspis-biovision/docs/scaleway-services.md",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            orasis = {
                "id": "aspis-biovision/Orasis/docs/adr/ADR-005-gpu-vm-large-files.md#chunk-0000",
                "file_id": "aspis-biovision/Orasis/docs/adr/ADR-005-gpu-vm-large-files.md",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 260,
                "text": "Orasis creates a GPU VM from image orasis-gpu-worker-v1 and deletes it after a 10 minute idle window.",
                "file_sorgente": "aspis-biovision/Orasis/docs/adr/ADR-005-gpu-vm-large-files.md",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([biovision["file_id"], orasis["file_id"]], [biovision, orasis])
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))
            captured = {}

            def fake_generate(prompt, *args, **_kwargs):
                captured["prompt"] = prompt
                return json.dumps(
                    {
                        "answer": "Biovision usa Serverless Containers con min_scale=0 e max_scale=10.",
                        "citations": [{"ref": "C1"}],
                        "not_found": False,
                        "suggested_path": None,
                    }
                )

            with patch("oracle.server.answerer.generate_with_openai_compatible", side_effect=fake_generate):
                answer = engine.ask("how does biovision limit gpu spawning in scaleway", limit=2, llm_config=self.API_LLM_CONFIG)

            self.assertFalse(answer["not_found"])
            self.assertIn("Biovision uses Scaleway", captured["prompt"])
            self.assertNotIn("orasis-gpu-worker", captured["prompt"])

    def test_ask_can_use_scaleway_remote_provider_with_privacy_gates(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite = SQLiteStore(root / "metadata.sqlite")
            vector_path = root / "empty-vectors.json"
            chunk = {
                "id": "aspis-biovision/docs/scaleway-services.md#chunk-0000",
                "file_id": "aspis-biovision/docs/scaleway-services.md",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 220,
                "text": "Biovision uses Scaleway Serverless Containers with min_scale=0 and no GPU required.",
                "file_sorgente": "aspis-biovision/docs/scaleway-services.md",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([chunk["file_id"]], [chunk])
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))
            captured = {}

            def fake_remote(prompt, config):
                captured["prompt"] = prompt
                captured["config"] = dict(config)
                return json.dumps(
                    {
                        "answer": "Biovision usa Serverless Containers con min_scale=0 e senza GPU.",
                        "citations": [{"ref": "C1"}],
                        "not_found": False,
                        "suggested_path": None,
                    }
                )

            with patch("oracle.server.answerer.generate_with_openai_compatible", side_effect=fake_remote):
                answer = engine.ask(
                    "how does biovision avoid scaleway gpu spawning",
                    limit=1,
                    llm_config={
                        "provider": "scaleway",
                        "model": "mistral-small-3.2-24b-instruct-2506",
                        "base_url": "https://api.scaleway.ai/v1/chat/completions",
                        "api_key": "sk-test-value",
                    },
                )

            self.assertFalse(answer["not_found"])
            self.assertEqual(answer["citations"][0]["file_source"], chunk["file_sorgente"])
            self.assertEqual(captured["config"]["provider"], "scaleway")
            self.assertIn("min_scale=0", captured["prompt"])

    def test_scaleway_request_normalizes_openai_compatible_chat_url(self):
        import sys
        from types import SimpleNamespace
        from unittest.mock import Mock

        from oracle.server.answerer import generate_with_openai_compatible

        class FakeResponse:
            def raise_for_status(self):
                return None

            def json(self):
                return {"choices": [{"message": {"content": "{\"answer\":\"ok\"}"}}]}

        config = {
            "provider": "scaleway",
            "model": "mistral-small-3.2-24b-instruct-2506",
            "base_url": "https://api.scaleway.ai/v1",
            "api_key": "sk-test-value",
        }
        post = Mock(return_value=FakeResponse())
        with patch.dict(sys.modules, {"httpx": SimpleNamespace(post=post)}):
            generate_with_openai_compatible("answer from context", config)

        self.assertEqual(post.call_args.args[0], "https://api.scaleway.ai/v1/chat/completions")
        request_body = post.call_args.kwargs["json"]
        self.assertEqual(request_body["model"], "mistral-small-3.2-24b-instruct-2506")

    def test_infomaniak_request_uses_json_schema_and_disables_thinking(self):
        import sys
        from types import SimpleNamespace
        from unittest.mock import Mock

        from oracle.server.answerer import generate_with_openai_compatible

        class FakeResponse:
            def raise_for_status(self):
                return None

            def json(self):
                return {"choices": [{"message": {"content": "{\"answer\":\"ok\"}"}}]}

        config = {
            "provider": "infomaniak",
            "model": "google/gemma-4-31B-it",
            "base_url": "https://api.infomaniak.com/2/ai/108646/openai/v1",
            "api_key": "ik-test-value",
        }
        post = Mock(return_value=FakeResponse())
        with patch.dict(sys.modules, {"httpx": SimpleNamespace(post=post)}):
            generate_with_openai_compatible("answer from context", config)

        self.assertEqual(
            post.call_args.args[0],
            "https://api.infomaniak.com/2/ai/108646/openai/v1/chat/completions",
        )
        request_body = post.call_args.kwargs["json"]
        self.assertEqual(request_body["response_format"]["type"], "json_schema")
        self.assertEqual(request_body["reasoning_effort"], "none")

    def test_remote_llm_missing_api_key_degrades_to_extractive(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite = SQLiteStore(root / "metadata.sqlite")
            vector_path = root / "empty-vectors.json"
            chunk = {
                "id": "oracle/server/routes.py#chunk-0000",
                "file_id": "oracle/server/routes.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 120,
                "text": "Oracle exposes /ask for local answer generation.",
                "file_sorgente": "oracle/server/routes.py",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            }
            sqlite.replace_chunks_for_files([chunk["file_id"]], [chunk])
            engine = QueryEngine(sqlite, LanceStore(vector_path), LanceStore(vector_path))

            # A missing API key is RECOVERABLE: Oracle degrades to an extractive,
            # retrieval-only answer (per plan: "no key -> extractive answers") — the
            # ONLY fallback. It must NOT raise. (The ZDR/GDPR gates were removed.)
            missing_key_answer = engine.ask(
                "how does oracle answer",
                limit=1,
                llm_config={
                    "provider": "scaleway",
                    "model": "mistral-small-3.2-24b-instruct-2506",
                    "base_url": "https://api.scaleway.ai/v1/chat/completions",
                    "api_key": "",
                },
            )
            self.assertNotEqual(missing_key_answer["answer_source"], "llm")
            self.assertIn("API key", missing_key_answer.get("fallback_reason", ""))

    def test_local_llm_providers_are_loopback_only_and_keyless(self):
        # omlx/ollama run on this machine: allowlisted, keyless, and FAIL-CLOSED
        # pinned to loopback (a non-loopback base_url raises — a "local" provider
        # pointing off-machine would exfiltrate code).
        from oracle.server.answerer import (
            OraclePrivacyGateError,
            normalize_llm_config,
            validate_remote_llm_config,
        )

        cfg = normalize_llm_config({"provider": "omlx", "model": "qwen"})
        self.assertEqual(cfg["base_url"], "http://127.0.0.1:8000/v1/chat/completions")
        validate_remote_llm_config(cfg)  # keyless local config is valid

        cfg = normalize_llm_config({"provider": "ollama", "model": "qwen"})
        self.assertEqual(cfg["base_url"], "http://127.0.0.1:11434/v1/chat/completions")
        validate_remote_llm_config(cfg)

        with self.assertRaises(OraclePrivacyGateError):
            validate_remote_llm_config(
                normalize_llm_config(
                    {
                        "provider": "omlx",
                        "model": "m",
                        "base_url": "http://evil.example.com:8000/v1",
                    }
                )
            )
        # Remote providers still require a key (unchanged).
        with self.assertRaises(RuntimeError):
            validate_remote_llm_config(
                normalize_llm_config({"provider": "scaleway", "model": "m"})
            )

    def test_non_allowlisted_provider_raises_fail_closed_not_extractive(self):
        # A non-allowlisted provider is a PRIVACY violation and must RAISE the
        # fail-closed exception — never be silently degraded to an extractive
        # answer (which would still have meant building a prompt for an un-vetted
        # endpoint). normalize_llm_config and the allowlist gate raise the SAME
        # type so the answer path can never catch/downgrade it. The provider
        # allowlist is the only privacy gate (ZDR/GDPR were removed).
        from oracle.server.answerer import (
            OraclePrivacyGateError,
            enforce_remote_llm_provider_allowlist,
            normalize_llm_config,
        )

        # normalize_llm_config raises the unified type for a bad provider.
        with self.assertRaises(OraclePrivacyGateError):
            normalize_llm_config({"provider": "openai"})
        # The allowlist gate raises the same type for a non-allowlisted provider.
        with self.assertRaises(OraclePrivacyGateError):
            enforce_remote_llm_provider_allowlist({"provider": "openai"})
        # The unified type is a RuntimeError subclass so existing RuntimeError
        # assertions stay green.
        self.assertTrue(issubclass(OraclePrivacyGateError, RuntimeError))

        chunks = [
            {
                "chunk_id": "oracle/server/answerer.py#chunk-0000",
                "file_source": "oracle/server/answerer.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 80,
                "retrieval": "lexical",
                "score": 50.0,
                "text": "Oracle answers are API-only and route through allowlisted providers.",
            }
        ]
        # End-to-end: a non-allowlisted provider must RAISE, not return extractive.
        with self.assertRaises(OraclePrivacyGateError):
            answer_from_context(
                "how does oracle answer",
                chunks,
                llm_config={
                    "provider": "openai",
                    "model": "gpt-4",
                    "api_key": "sk-secret",
                },
            )

    def test_privacy_gate_error_during_generation_is_not_degraded(self):
        # FIX 2: even if a privacy/allowlist violation surfaces from inside the
        # network call helper, the `except Exception` degrade-to-extractive path
        # in answer_with_llm_config must NOT swallow it — it re-raises the
        # fail-closed type. A generic generation error still degrades.
        from oracle.server.answerer import (
            OraclePrivacyGateError,
            answer_with_llm_config,
        )

        config = {
            "provider": "scaleway",
            "model": "mistral-small",
            "base_url": "https://api.scaleway.ai/v1/chat/completions",
            "api_key": "sk-secret",
        }
        context = [
            {
                "ref": "C1",
                "file_source": "x.py",
                "chunk_id": "x.py#chunk-0000",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 5,
                "retrieval": "lexical",
                "score": 1.0,
                "text": "hello world from the oracle corpus",
            }
        ]

        def raise_gate(*_a, **_k):
            raise OraclePrivacyGateError("Remote Oracle LLM provider is not allowlisted.")

        with patch(
            "oracle.server.answerer.generate_with_openai_compatible", side_effect=raise_gate
        ):
            with self.assertRaises(OraclePrivacyGateError):
                answer_with_llm_config("q", "prompt", context, config)

        # A generic (non-privacy) generation failure still DEGRADES to extractive.
        def raise_generic(*_a, **_k):
            raise RuntimeError("transient network blip")

        with patch(
            "oracle.server.answerer.generate_with_openai_compatible", side_effect=raise_generic
        ):
            answer = answer_with_llm_config("q", "prompt", context, config)
        self.assertNotEqual(answer.get("answer_source"), "llm")

    def test_oracle_index_job_runs_incremental_sync_prune_and_limited_dense_batch(self):
        from oracle.server.index_jobs import OracleIndexJobManager

        calls = []

        def fake_sync(root, sqlite_path, batch_files, progress, manifest_path=None, force=False):
            # P4: the text sync now receives the manifest (for the incremental
            # skip) and the `force` flag forwarded from run_once, so a warm run is
            # incremental and a forced reindex is a full rewrite.
            calls.append(("sync", str(root), batch_files, progress, force))
            return {"status": "complete", "files": 1, "chunks": 1, "skipped": 0}

        def fake_prune(root, sqlite_path, chunk_vector_path, manifest_path, progress):
            calls.append(("prune", str(root), progress))
            return {"status": "complete", "removed_files": 0}

        def fake_index(root, sqlite_path, chunk_vector_path, manifest_path, **kwargs):
            calls.append(("index", str(root), kwargs["max_batches"], kwargs["force"]))
            return {"status": "complete", "processed": 1, "pending": 0}

        with tempfile.TemporaryDirectory() as tmp:
            manager = OracleIndexJobManager()
            with patch("oracle.server.index_jobs.sync_text_chunks", side_effect=fake_sync), patch(
                "oracle.server.index_jobs.prune_excluded_chunks",
                side_effect=fake_prune,
            ), patch("oracle.server.index_jobs.index_file_chunks", side_effect=fake_index):
                result = manager.run_once(root=tmp, force=False, max_batches=2, idle=True)

        self.assertEqual(result["status"], "complete")
        self.assertEqual([call[0] for call in calls], ["sync", "prune", "index"])
        self.assertEqual(calls[2][2], 2)
        self.assertEqual(calls[2][3], False)
        # P4: the run-once `force` flag must reach the text sync so a warm
        # (force=False) run is incremental, not a full rewrite.
        self.assertEqual(calls[0][4], False)

    def test_background_index_job_failure_surfaces_error_status_and_logs(self):
        # FIX 1: a failure inside the background thread must NOT leave the job
        # stuck in "running" (which permanently disables the UI Index button)
        # and must NOT swallow the traceback silently. The job status must
        # transition to "error", the surfaced message must be path-free, and the
        # full traceback must be logged for debugging.
        from oracle.server.index_jobs import OracleIndexJobManager

        manager = OracleIndexJobManager()
        secret_path = "C:/Users/gualt/Desktop/aspis bio/secret.txt"

        def boom(*args, **kwargs):
            raise RuntimeError(f"torch exploded at {secret_path}")

        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(manager, "run_once", side_effect=boom), self.assertLogs(
                "oracle.server.index_jobs", level="ERROR"
            ) as captured:
                manager._background_target(
                    root=tmp, force=False, max_batches=None, idle=False
                )

        # The traceback (with the real path) is logged for debugging.
        self.assertTrue(
            any("torch exploded" in line for line in captured.output),
            captured.output,
        )
        # The job is no longer "running": the UI re-enables.
        self.assertIsNotNone(manager.job)
        self.assertEqual(manager.job["status"], "error")
        # The surfaced message never leaks the absolute path.
        self.assertNotIn(secret_path, manager.job.get("message", ""))
        self.assertNotIn("aspis bio", manager.job.get("message", ""))

    def test_manual_index_params_run_immediately_and_unbounded(self):
        # FIX 2: the manual "Index now" must run unconditionally (idle=false so
        # it is never deferred by the high idle RAM floor) and must process all
        # pending files (unbounded batches), not just a single batch.
        from oracle.server.index_jobs import resolve_index_run_params

        manual = resolve_index_run_params(manual=True, max_batches=1, idle=True)
        self.assertEqual(manual["idle"], False)
        # Unbounded (None) so the whole workspace is indexed, not one batch.
        self.assertIsNone(manual["max_batches"])

        # The AUTO warm/watch path (manual=False) keeps the caller-provided
        # idle-deferred, single-batch behavior unchanged.
        auto = resolve_index_run_params(manual=False, max_batches=1, idle=True)
        self.assertEqual(auto["idle"], True)
        self.assertEqual(auto["max_batches"], 1)

    def test_oracle_index_status_uses_frontend_safe_camel_case(self):
        from oracle.server.index_jobs import OracleIndexJobManager

        with tempfile.TemporaryDirectory() as tmp:
            status = OracleIndexJobManager().status(root=tmp)

        self.assertIn("watcherRunning", status)
        self.assertNotIn("watcher_running", status)
        self.assertIn("expectedFiles", status["index"])
        self.assertIn("pendingFiles", status["index"])
        self.assertNotIn("expected_files", status["index"])

    def test_manifest_indexed_files_returns_relative_paths_and_shape(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("a.ts", "b.ts", "c.ts"):
                source = root / "src" / name
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text(f"export const {name[0]} = 'Cloudflare Worker';\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            index_file_chunks(
                root,
                sqlite_path,
                chunk_vectors,
                manifest_path=manifest,
                min_free_gb=0,
                use_sentence_transformer=False,
            )

            result = manifest_indexed_files(root, limit=100, offset=0, manifest_path=manifest)

            self.assertEqual(result["total"], 3)
            self.assertEqual(result["limit"], 100)
            self.assertEqual(result["offset"], 0)
            self.assertEqual([f["path"] for f in result["files"]], ["src/a.ts", "src/b.ts", "src/c.ts"])
            first = result["files"][0]
            self.assertEqual(set(first), {"path", "chunks", "updatedAt"})
            self.assertGreaterEqual(first["chunks"], 1)
            self.assertTrue(first["updatedAt"])
            # PRIVACY: never an absolute path.
            self.assertFalse(Path(first["path"]).is_absolute())
            self.assertNotIn(tmp, first["path"])

    def test_manifest_indexed_files_respects_limit_offset_and_filter(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("a.ts", "b.ts", "c.ts"):
                source = root / "src" / name
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text(f"export const {name[0]} = 'Worker';\n", encoding="utf-8")
            extra = root / "docs" / "readme.md"
            extra.parent.mkdir(parents=True, exist_ok=True)
            extra.write_text("# Cloudflare Worker docs\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"
            index_file_chunks(
                root, sqlite_path, chunk_vectors, manifest_path=manifest,
                min_free_gb=0, use_sentence_transformer=False,
            )

            page = manifest_indexed_files(root, limit=2, offset=1, manifest_path=manifest)
            self.assertEqual(page["total"], 4)
            self.assertEqual(page["limit"], 2)
            self.assertEqual(page["offset"], 1)
            self.assertEqual([f["path"] for f in page["files"]], ["src/a.ts", "src/b.ts"])

            filtered = manifest_indexed_files(root, filter_substr="readme", manifest_path=manifest)
            self.assertEqual(filtered["total"], 1)
            self.assertEqual(filtered["files"][0]["path"], "docs/readme.md")

    def test_manifest_indexed_files_caps_limit(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = manifest_indexed_files(root, limit=10_000, offset=-5, manifest_path=root / "manifest.json")
            self.assertEqual(result["limit"], MAX_INDEXED_FILES_LIMIT)
            self.assertEqual(result["offset"], 0)
            self.assertEqual(result["total"], 0)
            self.assertEqual(result["files"], [])

    def test_manifest_indexed_files_caches_parse_until_manifest_changes(self):
        # FIX 3: a search-as-you-type UI hits /index/files repeatedly on the same
        # manifest version; repeated reads must parse the file only ONCE, and a
        # manifest change (new mtime) must invalidate the cache and reparse.
        import oracle.ingestion.chunk_index as chunk_index_mod

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "src" / "worker.ts"
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text("export const w = 'Cloudflare Worker';\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"
            index_file_chunks(
                root, sqlite_path, chunk_vectors, manifest_path=manifest,
                min_free_gb=0, use_sentence_transformer=False,
            )

            # Clear any cache state leaked from prior tests in this process so the
            # parse count is deterministic for THIS manifest.
            chunk_index_mod._MANIFEST_PARSE_CACHE.clear()

            real_load_manifest = chunk_index_mod.load_manifest
            calls = {"n": 0}

            def counting_load_manifest(path):
                calls["n"] += 1
                return real_load_manifest(path)

            with patch.object(chunk_index_mod, "load_manifest", side_effect=counting_load_manifest):
                first = manifest_indexed_files(root, manifest_path=manifest)
                second = manifest_indexed_files(root, manifest_path=manifest)
                # Two reads on an unchanged manifest parse the file exactly once.
                self.assertEqual(calls["n"], 1)
                self.assertEqual(first["total"], second["total"])

                # Mutate the manifest: a new mtime_ns must invalidate the cache.
                os.utime(manifest, ns=(0, 1))  # force a distinct mtime_ns
                manifest_indexed_files(root, manifest_path=manifest)
                self.assertEqual(calls["n"], 2)

    def test_manifest_parse_cache_is_bounded(self):
        # FIX 3: the parse cache is bounded to the last few manifests (LRU).
        import oracle.ingestion.chunk_index as chunk_index_mod

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            chunk_index_mod._MANIFEST_PARSE_CACHE.clear()
            for i in range(chunk_index_mod._MANIFEST_PARSE_CACHE_MAX + 3):
                manifest = root / f"manifest_{i}.json"
                manifest.write_text('{"files": {}}', encoding="utf-8")
                manifest_indexed_files(root, manifest_path=manifest)
            self.assertLessEqual(
                len(chunk_index_mod._MANIFEST_PARSE_CACHE),
                chunk_index_mod._MANIFEST_PARSE_CACHE_MAX,
            )

    def test_start_watcher_creates_at_most_one_observer_under_concurrency(self):
        # FIX 2: two concurrent start_watcher() calls must build AT MOST one
        # observer (no orphaned watchdog thread firing redundant index jobs).
        import threading
        from oracle.server.index_jobs import OracleIndexJobManager

        manager = OracleIndexJobManager()
        call_count = {"n": 0}
        count_lock = threading.Lock()
        ready = threading.Barrier(2)

        class _FakeObserver:
            def stop(self):
                pass

            def join(self, timeout=None):
                pass

        def slow_start_watching(_on_batch_ready, _root):
            with count_lock:
                call_count["n"] += 1
            return _FakeObserver()

        with tempfile.TemporaryDirectory() as tmp:
            with patch("oracle.server.index_jobs.start_watching", side_effect=slow_start_watching):
                def worker():
                    ready.wait()
                    manager.start_watcher(root=tmp)

                threads = [threading.Thread(target=worker) for _ in range(2)]
                for t in threads:
                    t.start()
                for t in threads:
                    t.join()

        self.assertEqual(
            call_count["n"], 1,
            "concurrent start_watcher calls must invoke start_watching exactly once",
        )

    def test_oracle_data_directory_is_excluded_from_indexing(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # The index output lives under <root>/oracle-data/. The indexer must
            # never index its own LanceDB/SQLite/manifest output (growth loop).
            for rel in (
                "oracle-data/metadata.sqlite",
                "oracle-data/chunks.lancedb",
                "oracle-data/chunk-index-manifest.json",
                "oracle-data/venv/lib/site.py",
            ):
                self.assertFalse(
                    chunk_path_allowed(root / rel, root),
                    f"{rel} must be excluded from indexing",
                )
            # A normal source file is still allowed.
            self.assertTrue(chunk_path_allowed(root / "src" / "worker.ts", root))

    def test_chunk_index_pauses_before_embedding_when_ram_guard_trips(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "src" / "worker.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export const gpuPolicy = 'limit Scaleway GPU spawning';\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            # Persistent low RAM: the in-batch guard trips and the wait-and-retry
            # helper still reports a shortfall after its cycles -> genuine
            # give-up: paused_low_memory. Patch the recovery helper so the test
            # does not sleep; it reports RAM never recovered (0.5 < 1.0 floor).
            with patch(
                "oracle.ingestion.chunk_index.free_memory_gb",
                side_effect=[10.0, 10.0, 0.5],
            ), patch(
                "oracle.ingestion.chunk_index.wait_for_memory_recovery",
                return_value=0.5,
            ) as recovery:
                result = index_file_chunks(
                    root,
                    sqlite_path,
                    chunk_vectors,
                    manifest_path=manifest,
                    min_free_gb=1.0,
                    use_sentence_transformer=False,
                )

            recovery.assert_called_once_with(1.0, False)
            self.assertEqual(result["status"], "paused_low_memory")
            self.assertEqual(result["processed"], 0)
            self.assertEqual(LanceStore(chunk_vectors).count(), 0)
            self.assertEqual(SQLiteStore(sqlite_path).chunk_count(), 0)

    def test_chunk_index_waits_and_resumes_when_ram_recovers(self):
        # Low RAM is transient: the in-batch guard trips, the wait-and-retry
        # helper reports RAM recovered, and the loop indexes the file instead of
        # returning paused_low_memory.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "src" / "worker.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export const ok = 'ram recovered';\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            # pre-scan=10, pre-batch=10, then the in-batch read trips at 0.5; once
            # the recovery helper has run (it returns 9.0) every later read is
            # plentiful so the run completes.
            ram_reads = iter([10.0, 10.0, 0.5])

            def fake_free_ram():
                try:
                    return next(ram_reads)
                except StopIteration:
                    return 9.0

            with patch(
                "oracle.ingestion.chunk_index.free_memory_gb",
                side_effect=fake_free_ram,
            ), patch(
                "oracle.ingestion.chunk_index.wait_for_memory_recovery",
                return_value=9.0,
            ) as recovery:
                result = index_file_chunks(
                    root,
                    sqlite_path,
                    chunk_vectors,
                    manifest_path=manifest,
                    min_free_gb=1.0,
                    use_sentence_transformer=False,
                )

            recovery.assert_called_once_with(1.0, False)
            self.assertEqual(result["status"], "complete")
            self.assertEqual(result["processed"], 1)
            self.assertGreater(SQLiteStore(sqlite_path).chunk_count(), 0)

    def test_chunk_index_pauses_before_embedding_when_gpu_is_hot(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "src" / "worker.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export const gpuPolicy = 'keep laptop thermals safe';\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            # GPU stays hot through the whole cooldown (cooldown EXHAUSTS its
            # cycles) -> genuine give-up: paused_gpu_temperature. Patch the
            # cooldown helper so the test does not sleep for real; it reports the
            # GPU is still at/above the ceiling.
            with patch("oracle.ingestion.chunk_index.gpu_temperature_c", return_value=83), patch(
                "oracle.ingestion.chunk_index.wait_for_gpu_cooldown", return_value=83
            ) as cooldown:
                result = index_file_chunks(
                    root,
                    sqlite_path,
                    chunk_vectors,
                    manifest_path=manifest,
                    min_free_gb=0,
                    max_gpu_temp_c=80,
                    use_sentence_transformer=False,
                )

            cooldown.assert_called_once_with(80, False)
            self.assertEqual(result["status"], "paused_gpu_temperature")
            self.assertEqual(result["processed"], 0)
            self.assertEqual(result["gpu_temp_c"], 83)
            self.assertEqual(LanceStore(chunk_vectors).count(), 0)

    def test_chunk_index_cools_and_resumes_when_gpu_recovers(self):
        # The GPU trips the thermal ceiling on the FIRST batch, then the cooldown
        # helper reports it cooled below the ceiling. The loop must NOT return
        # paused_gpu_temperature: it must resume and index every pending file.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("a", "b", "c"):
                source = root / "src" / f"{name}.ts"
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text(
                    f"export const {name} = 'keep laptop thermals safe';\n",
                    encoding="utf-8",
                )
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            # Hot on the first temperature read, cool (below ceiling) on every
            # subsequent read. The cooldown helper is patched to report a cooled
            # temperature so the loop continues; no real sleep.
            temps = iter([90] + [60] * 50)

            def fake_temp():
                try:
                    return next(temps)
                except StopIteration:
                    return 60

            with patch("oracle.ingestion.chunk_index.gpu_temperature_c", side_effect=fake_temp), patch(
                "oracle.ingestion.chunk_index.wait_for_gpu_cooldown", return_value=60
            ) as cooldown, patch("oracle.ingestion.chunk_index.time.sleep") as sleeper:
                result = index_file_chunks(
                    root,
                    sqlite_path,
                    chunk_vectors,
                    manifest_path=manifest,
                    batch_files=1,
                    min_free_gb=0,
                    max_gpu_temp_c=80,
                    use_sentence_transformer=False,
                )

            self.assertEqual(result["status"], "complete")
            self.assertEqual(result["processed"], 3)
            self.assertNotEqual(result["status"], "paused_gpu_temperature")
            cooldown.assert_called_once_with(80, False)
            sleeper.assert_not_called()  # the loop itself must not sleep
            self.assertGreater(SQLiteStore(sqlite_path).chunk_count(), 0)

    def test_chunk_index_emits_cooling_then_running_phase_on_gpu_event(self):
        # When the GPU trips the ceiling and then cools, index_file_chunks must
        # fire on_phase("cooling_gpu", {...temp...}) before the wait and
        # on_phase("running", {}) on resume — the live sub-state the UI shows so
        # a cool-and-resume reads as "working, not stuck".
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "src" / "a.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export const a = 'keep thermals safe';\n", encoding="utf-8")
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"

            temps = iter([90] + [60] * 50)

            def fake_temp():
                try:
                    return next(temps)
                except StopIteration:
                    return 60

            phases: list[tuple[str, dict]] = []

            with patch("oracle.ingestion.chunk_index.gpu_temperature_c", side_effect=fake_temp), patch(
                "oracle.ingestion.chunk_index.wait_for_gpu_cooldown", return_value=60
            ):
                result = index_file_chunks(
                    root,
                    sqlite_path,
                    chunk_vectors,
                    manifest_path=manifest,
                    batch_files=1,
                    min_free_gb=0,
                    max_gpu_temp_c=80,
                    use_sentence_transformer=False,
                    on_phase=lambda phase, detail: phases.append((phase, detail)),
                )

            self.assertEqual(result["status"], "complete")
            names = [phase for phase, _ in phases]
            self.assertIn("cooling_gpu", names)
            # cooling must come before the subsequent running resume.
            cooling_idx = names.index("cooling_gpu")
            self.assertIn("running", names[cooling_idx + 1 :])
            cooling_detail = phases[cooling_idx][1]
            self.assertEqual(cooling_detail["gpu_temp_c"], 90)
            self.assertEqual(cooling_detail["max_gpu_temp_c"], 80)
            # Privacy: phase detail carries numbers only, never a path.
            self.assertNotIn("root", cooling_detail)

    def test_run_once_exposes_cooling_then_running_phase_in_status(self):
        # The job manager must surface the live phase + a path-free phaseMessage
        # via /index/status's `job` object while the run is still "running", then
        # clear back to running on resume — so the UI can show
        # "GPU cooling (90°C), resuming…".
        from oracle.server.index_jobs import OracleIndexJobManager

        captured: list[dict] = []

        def fake_index(root, sqlite_path, chunk_vector_path, manifest_path, **kwargs):
            on_phase = kwargs["on_phase"]
            on_phase("cooling_gpu", {"gpu_temp_c": 90, "max_gpu_temp_c": 80})
            captured.append(self._job_phase_snapshot(manager))
            on_phase("running", {})
            captured.append(self._job_phase_snapshot(manager))
            return {"status": "complete", "processed": 1, "pending": 0}

        with tempfile.TemporaryDirectory() as tmp:
            manager = OracleIndexJobManager()
            with patch(
                "oracle.server.index_jobs.sync_text_chunks",
                return_value={"status": "complete"},
            ), patch(
                "oracle.server.index_jobs.prune_excluded_chunks",
                return_value={"status": "complete"},
            ), patch(
                "oracle.server.index_jobs.index_file_chunks", side_effect=fake_index
            ):
                result = manager.run_once(root=tmp, force=False, max_batches=2, idle=True)

        self.assertEqual(result["status"], "complete")
        # During cooling: job.phase == cooling_gpu + a path-free message.
        cooling = captured[0]
        self.assertEqual(cooling["status"], "running")
        self.assertEqual(cooling["phase"], "cooling_gpu")
        self.assertEqual(cooling["phaseMessage"], "GPU cooling (90°C), resuming…")
        self.assertNotIn(tmp, cooling["phaseMessage"])
        self.assertEqual(cooling["gpu_temp_c"], 90)
        # On resume: phase flips back to running and the message clears.
        running = captured[1]
        self.assertEqual(running["phase"], "running")
        self.assertNotIn("phaseMessage", running)

    def test_run_once_exposes_waiting_memory_phase_message(self):
        from oracle.server.index_jobs import OracleIndexJobManager

        captured: list[dict] = []

        def fake_index(root, sqlite_path, chunk_vector_path, manifest_path, **kwargs):
            kwargs["on_phase"]("waiting_memory", {"free_gb": 1.4, "min_free_gb": 4.0})
            captured.append(self._job_phase_snapshot(manager))
            return {"status": "complete", "processed": 1, "pending": 0}

        with tempfile.TemporaryDirectory() as tmp:
            manager = OracleIndexJobManager()
            with patch(
                "oracle.server.index_jobs.sync_text_chunks",
                return_value={"status": "complete"},
            ), patch(
                "oracle.server.index_jobs.prune_excluded_chunks",
                return_value={"status": "complete"},
            ), patch(
                "oracle.server.index_jobs.index_file_chunks", side_effect=fake_index
            ):
                manager.run_once(root=tmp, force=False, max_batches=2, idle=True)

        waiting = captured[0]
        self.assertEqual(waiting["phase"], "waiting_memory")
        self.assertEqual(
            waiting["phaseMessage"], "Waiting for memory (1.4 GB free), resuming…"
        )

    def _job_phase_snapshot(self, manager) -> dict:
        # Read the live phase exactly as /index/status would expose it (camelCase
        # phaseMessage), without depending on chunk_index_status hitting disk.
        with manager.lock:
            job = dict(manager.job) if manager.job else {"status": "idle"}
        if "phase_message" in job:
            job["phaseMessage"] = job.pop("phase_message")
        return job

    def test_prune_excluded_chunks_removes_generated_noise(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"
            noisy_file = "test-results/.last-run.json"
            useful_file = "src/App.tsx"
            useful_path = root / useful_file
            useful_path.parent.mkdir(parents=True)
            useful_path.write_text("export const app = true;\n", encoding="utf-8")
            sqlite = SQLiteStore(sqlite_path)
            sqlite.replace_chunks_for_files(
                [noisy_file, useful_file],
                [
                    {
                        "id": f"{noisy_file}#chunk-0000",
                        "file_id": noisy_file,
                        "chunk_index": 0,
                        "start_char": 0,
                        "end_char": 2,
                        "text": "{}",
                        "file_sorgente": noisy_file,
                        "ultima_modifica": "2026-05-28T00:00:00Z",
                        "embedding_dims": 1024,
                    },
                    {
                        "id": f"{useful_file}#chunk-0000",
                        "file_id": useful_file,
                        "chunk_index": 0,
                        "start_char": 0,
                        "end_char": 25,
                        "text": "export const app = true;",
                        "file_sorgente": useful_file,
                        "ultima_modifica": "2026-05-28T00:00:00Z",
                        "embedding_dims": 1024,
                    },
                ],
            )
            LanceStore(chunk_vectors).replace_all(
                [
                    {"id": f"{noisy_file}#chunk-0000", "label": "noise", "area": "FileChunk", "cluster_semantic": "json", "vector": [1.0, 0.0]},
                    {"id": f"{useful_file}#chunk-0000", "label": "app", "area": "FileChunk", "cluster_semantic": "ts", "vector": [0.0, 1.0]},
                ]
            )
            manifest.write_text(
                json.dumps({"root": str(root.resolve()), "files": {noisy_file: {}, useful_file: {}}}),
                encoding="utf-8",
            )

            result = prune_excluded_chunks(root, sqlite_path, chunk_vectors, manifest)

            self.assertEqual(result["removed_files"], 1)
            self.assertIsNone(sqlite.get_chunk(f"{noisy_file}#chunk-0000"))
            self.assertIsNotNone(sqlite.get_chunk(f"{useful_file}#chunk-0000"))
            self.assertEqual(LanceStore(chunk_vectors).count(), 1)

    def test_prune_excluded_chunks_removes_orphan_vectors(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "metadata.sqlite"
            chunk_vectors = root / "chunks.json"
            manifest = root / "manifest.json"
            useful_file = "src/App.tsx"
            useful_path = root / useful_file
            useful_path.parent.mkdir(parents=True)
            useful_path.write_text("export const app = true;\n", encoding="utf-8")
            sqlite = SQLiteStore(sqlite_path)
            sqlite.replace_chunks_for_files(
                [useful_file],
                [
                    {
                        "id": f"{useful_file}#chunk-0000",
                        "file_id": useful_file,
                        "chunk_index": 0,
                        "start_char": 0,
                        "end_char": 25,
                        "text": "export const app = true;",
                        "file_sorgente": useful_file,
                        "ultima_modifica": "2026-05-28T00:00:00Z",
                        "embedding_dims": 1024,
                    },
                ],
            )
            LanceStore(chunk_vectors).replace_all(
                [
                    {"id": f"{useful_file}#chunk-0000", "label": "app", "area": "FileChunk", "cluster_semantic": "ts", "vector": [0.0, 1.0]},
                    {"id": f"{useful_file}#chunk-9999", "label": "orphan", "area": "FileChunk", "cluster_semantic": "ts", "vector": [1.0, 0.0]},
                ]
            )
            manifest.write_text(
                json.dumps({"root": str(root.resolve()), "files": {useful_file: {}}}),
                encoding="utf-8",
            )

            result = prune_excluded_chunks(root, sqlite_path, chunk_vectors, manifest)

            self.assertEqual(result["removed_vectors"], 1)
            self.assertEqual(LanceStore(chunk_vectors).count(), 1)

    def test_lancedb_vector_store_backend_round_trips_records(self):
        try:
            import lancedb  # noqa: F401
        except Exception:
            self.skipTest("lancedb is not installed")

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "vectors.lancedb"
            store = LanceStore(path)
            store.replace_all(
                [
                    {
                        "id": "src/App.tsx",
                        "label": "App.tsx",
                        "area": "Browser",
                        "cluster_semantic": "UI",
                        "vector": [1.0, 0.0, 0.0],
                    },
                    {
                        "id": "oracle/server/main.py",
                        "label": "main.py",
                        "area": "Oracle",
                        "cluster_semantic": "Oracle",
                        "vector": [0.0, 1.0, 0.0],
                    },
                ]
            )

            self.assertEqual(store.backend, "lancedb")
            self.assertEqual(store.count(), 2)
            self.assertEqual(store.similar("src/App.tsx", 1)[0]["id"], "oracle/server/main.py")

    def test_empty_vector_store_search_does_not_load_query_embedder(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = LanceStore(Path(tmp) / "vectors.json")

            with patch("oracle.store.lance_store.embed_query_text") as embed_query:
                embed_query.side_effect = AssertionError("query embedder should not be loaded")

                self.assertEqual(store.search("scaleway gpu", 3), [])
                embed_query.assert_not_called()

    def test_lance_search_prefixes_queries_for_semantic_embedding_profile(self):
        with tempfile.TemporaryDirectory() as tmp:
            store = LanceStore(Path(tmp) / "vectors.json")
            store.replace_all(
                [
                    {
                        "id": "src-tauri/src/backend/commands.rs#chunk-0000",
                        "label": "commands.rs chunk 1",
                        "area": "FileChunk",
                        "cluster_semantic": "rs",
                        "vector": [1.0, 0.0],
                    }
                ]
            )

            with patch.dict(os.environ, {"ORACLE_QUERY_PROFILE": "semantic-prefix-v2"}), patch(
                "oracle.store.lance_store.embed_query_text",
                return_value=[1.0, 0.0],
            ) as embed_query:
                result = store.search("where is Cloudflare worker secret rotation implemented", 1)

            self.assertEqual(result[0]["id"], "src-tauri/src/backend/commands.rs#chunk-0000")
            called_query = embed_query.call_args.args[0]
            self.assertIn("TASK: retrieve Aspis Bio", called_query)
            self.assertIn("QUERY: where is Cloudflare worker secret rotation implemented", called_query)
            self.assertIn("cloudflare_worker_secret_rotation", called_query)

    def test_runtime_status_reports_vector_backend_and_ollama_shape(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "vectors.json"
            LanceStore(path).replace_all(
                [
                    {
                        "id": "a",
                        "label": "a",
                        "area": "Codebase",
                        "cluster_semantic": "Test",
                        "vector": [1.0, 0.0],
                    }
                ]
            )

            status = runtime_status(path)

            self.assertEqual(status["vector_store"]["backend"], "json")
            self.assertTrue(status["vector_store"]["ready"])
            # Vestigial ollama status remains in the payload shape but is always
            # disabled (local chat path removed; answers are API-only), and no
            # local-LLM setup commands are emitted anymore.
            self.assertIn("server", status["ollama"])
            self.assertIn("model_available", status["ollama"])
            self.assertFalse(status["ollama"]["model_available"])
            self.assertEqual(status["setup_commands"], [])

    def test_runtime_readiness_derives_from_chunk_store_not_vector_store(self):
        # The live-probe scenario that broke the UI: the legacy node-level
        # vector_store (vectors.*) is EMPTY while the real chunk store
        # (chunks.* + the SQLite chunk table) is fully populated. Readiness must
        # come from the chunk store: top-level `ready` and `chunk_store.ready`
        # are True even though `vector_store.ready` is False.
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            vector_path = tmp / "vectors.json"  # EMPTY legacy node store
            chunk_vector_path = tmp / "chunks.json"
            sqlite_path = tmp / "metadata.sqlite"
            manifest_path = tmp / "chunk-index-manifest.json"

            # Populate ONLY the chunk store: one chunk vector + one SQLite chunk.
            LanceStore(chunk_vector_path).replace_all(
                [
                    {
                        "id": "f.py#chunk-0000",
                        "label": "f.py",
                        "area": "Codebase",
                        "cluster_semantic": "Test",
                        "vector": [1.0, 0.0],
                    }
                ]
            )
            store = SQLiteStore(sqlite_path)
            store.replace_all_chunks(
                [
                    {
                        "id": "f.py#chunk-0000",
                        "file_id": "f.py",
                        "chunk_index": 0,
                        "start_char": 0,
                        "end_char": 10,
                        "text": "healthcheck",
                        "file_sorgente": "f.py",
                        "ultima_modifica": "2026-01-01T00:00:00Z",
                        "embedding_dims": 2,
                    }
                ]
            )

            with patch.multiple(
                "oracle.verify_runtime",
                CHUNK_DB_PATH=chunk_vector_path,
                SQLITE_PATH=sqlite_path,
                CHUNK_MANIFEST_PATH=manifest_path,
            ):
                status = runtime_status(vector_path)

            # Legacy node-level vector store is empty / not ready...
            self.assertEqual(status["vector_store"]["records"], 0)
            self.assertFalse(status["vector_store"]["ready"])
            # ...but the chunk store (the real index) is ready with real counts.
            self.assertEqual(status["chunk_store"]["records"], 1)
            self.assertEqual(status["chunk_store"]["files"], 1)
            self.assertEqual(status["chunk_store"]["vector_records"], 1)
            self.assertTrue(status["chunk_store"]["ready"])
            # Top-level readiness mirrors the chunk store, NOT the vector store.
            self.assertTrue(status["ready"])

    def test_runtime_not_ready_when_chunk_store_empty(self):
        # A genuinely empty workspace: both stores empty -> not ready.
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            vector_path = tmp / "vectors.json"
            chunk_vector_path = tmp / "chunks.json"
            sqlite_path = tmp / "metadata.sqlite"
            manifest_path = tmp / "chunk-index-manifest.json"
            SQLiteStore(sqlite_path)  # empty schema, zero chunks

            with patch.multiple(
                "oracle.verify_runtime",
                CHUNK_DB_PATH=chunk_vector_path,
                SQLITE_PATH=sqlite_path,
                CHUNK_MANIFEST_PATH=manifest_path,
            ):
                status = runtime_status(vector_path)

            self.assertFalse(status["chunk_store"]["ready"])
            self.assertFalse(status["ready"])

    def test_model_names_accepts_dict_and_object_shapes(self):
        class Model:
            model = "qwen3.5:4b"

        self.assertEqual(model_names({"models": [{"name": "a"}, {"model": "b"}]}), {"a", "b"})
        self.assertEqual(model_names(type("Payload", (), {"models": [Model()]})()), {"qwen3.5:4b"})

    def test_ollama_placeholder_card_is_rejected(self):
        self.assertTrue(
            is_placeholder_card(
                {
                    "funzione_primaria": "descrizione in max 2 righe",
                    "espone_api": ["endpoint o metodi pubblici"],
                    "dipende_da": ["dipendenze esterne rilevanti"],
                    "tecnologie": ["tecnologie/librerie usate"],
                }
            )
        )

    def test_resolve_min_free_gb_uses_low_floor_only_on_cuda(self):
        # Only CUDA keeps the model in a SEPARATE VRAM pool, so only CUDA gets
        # the low system-RAM floor. "mps" is Apple unified memory — the model
        # and activations live in system RAM, so it must keep the conservative
        # CPU floors or the paused_low_memory backpressure never fires (this
        # froze a 64GB M1 Max). CPU keeps the existing behavior: high floor
        # when idle (defer on a busy machine), normal floor when requested.
        self.assertEqual(
            resolve_min_free_gb("cuda", idle=True), oracle_config.CHUNK_GPU_MIN_FREE_GB
        )
        self.assertEqual(
            resolve_min_free_gb("cuda", idle=False), oracle_config.CHUNK_GPU_MIN_FREE_GB
        )
        self.assertEqual(
            resolve_min_free_gb("mps", idle=True), max(oracle_config.CHUNK_MIN_FREE_GB, 8.0)
        )
        self.assertEqual(
            resolve_min_free_gb("mps", idle=False), oracle_config.CHUNK_MIN_FREE_GB
        )

        self.assertEqual(
            resolve_min_free_gb("cpu", idle=True), max(oracle_config.CHUNK_MIN_FREE_GB, 8.0)
        )
        self.assertEqual(
            resolve_min_free_gb("cpu", idle=False), oracle_config.CHUNK_MIN_FREE_GB
        )
        # Unknown/None device is treated conservatively as CPU.
        self.assertEqual(
            resolve_min_free_gb(None, idle=False), oracle_config.CHUNK_MIN_FREE_GB
        )

    def test_adaptive_batch_files_scales_with_free_ram(self):
        # Owner request: grow while RAM is plentiful, shrink before the floor
        # pauses us, hold in between; cap 4x base, floor max(2, base//4).
        from oracle.ingestion.chunk_index import adaptive_batch_files

        base, floor = 16, 5.0
        # plenty (>= 4x floor): doubles, capped at 4x base.
        self.assertEqual(adaptive_batch_files(base, 16, 40.0, floor), 32)
        self.assertEqual(adaptive_batch_files(base, 32, 40.0, floor), 64)
        self.assertEqual(adaptive_batch_files(base, 64, 40.0, floor), 64)
        # comfortable middle: holds.
        self.assertEqual(adaptive_batch_files(base, 32, 15.0, floor), 32)
        # tight (< 2x floor): halves, floored.
        self.assertEqual(adaptive_batch_files(base, 32, 9.0, floor), 16)
        self.assertEqual(adaptive_batch_files(base, 4, 9.0, floor), 4)
        self.assertEqual(adaptive_batch_files(base, 2, 9.0, floor), 4)
        # floor disabled: no signal, hold.
        self.assertEqual(adaptive_batch_files(base, 24, 1.0, 0.0), 24)

    def test_darwin_effective_free_gb_zeroes_under_memory_pressure(self):
        # vm_stat's free+inactive+speculative+purgeable OVERSTATES availability
        # under load (~17GB "free" while swapping 30GB). When the kernel itself
        # reports memory pressure (level >= 2 = warning/critical), the index
        # loop must see 0.0 and pause. Pure over the parsed level.
        from oracle.ingestion.chunk_index import darwin_effective_free_gb

        # critical -> hard pause.
        self.assertEqual(darwin_effective_free_gb(17.0, 0.01, 4), 0.0)
        # warning -> trust only the genuinely FREE pages: ~0 during a real
        # thrash (pause), tens of GB on a recovered machine whose kernel level
        # is still sticky at 2 (proceed) — the live false-freeze of 2026-06-12.
        self.assertEqual(darwin_effective_free_gb(17.0, 0.01, 2), 0.01)
        self.assertEqual(darwin_effective_free_gb(40.0, 24.6, 2), 24.6)
        # normal / unknown -> the full vm_stat estimate.
        self.assertEqual(darwin_effective_free_gb(17.0, 5.0, 1), 17.0)
        self.assertEqual(darwin_effective_free_gb(17.0, 5.0, None), 17.0)

    def test_choose_device_matrix_respects_override_and_vram_floor(self):
        threshold = oracle_config.MIN_GPU_FREE_GB
        # Explicit override always wins, even when nothing is available.
        self.assertEqual(
            choose_device(
                cuda_available=False, free_vram_gb=None, mps_available=False, override="cuda"
            ),
            "cuda",
        )
        self.assertEqual(
            choose_device(
                cuda_available=True, free_vram_gb=0.5, mps_available=False, override="cpu"
            ),
            "cpu",
        )
        # CUDA with enough free VRAM -> cuda.
        self.assertEqual(
            choose_device(
                cuda_available=True,
                free_vram_gb=threshold + 1.0,
                mps_available=False,
                override="",
            ),
            "cuda",
        )
        # CUDA with too little free VRAM -> mps if present, else cpu.
        self.assertEqual(
            choose_device(
                cuda_available=True,
                free_vram_gb=threshold - 1.0,
                mps_available=True,
                override="",
            ),
            "mps",
        )
        self.assertEqual(
            choose_device(
                cuda_available=True,
                free_vram_gb=threshold - 1.0,
                mps_available=False,
                override="",
            ),
            "cpu",
        )
        # No CUDA, MPS available -> mps. Nothing -> cpu.
        self.assertEqual(
            choose_device(
                cuda_available=False, free_vram_gb=None, mps_available=True, override=""
            ),
            "mps",
        )
        self.assertEqual(
            choose_device(
                cuda_available=False, free_vram_gb=None, mps_available=False, override=""
            ),
            "cpu",
        )
        # Unknown free VRAM (mem_get_info failed) with CUDA available is treated
        # as insufficient -> do NOT risk an OOM, fall back.
        self.assertEqual(
            choose_device(
                cuda_available=True, free_vram_gb=None, mps_available=False, override=""
            ),
            "cpu",
        )

    def test_choose_device_mps_gates_on_unified_free(self):
        """MPS is pre-emptively diverted to CPU when free unified memory is KNOWN
        to be below the GPU floor, but stays MPS when memory is sufficient or
        unknown (None) — the resident embedder loads once and the index burst
        keeps the GPU; only positive evidence of low memory diverts to CPU."""
        threshold = oracle_config.MIN_GPU_FREE_GB
        # MPS available, free unified memory sufficient -> "mps".
        self.assertEqual(
            choose_device(
                cuda_available=False, free_vram_gb=None, mps_available=True,
                override="", free_unified_gb=threshold + 1.0,
            ),
            "mps",
        )
        # MPS available, free unified memory below floor -> "cpu".
        self.assertEqual(
            choose_device(
                cuda_available=False, free_vram_gb=None, mps_available=True,
                override="", free_unified_gb=threshold - 1.0,
            ),
            "cpu",
        )
        # MPS available, free unified memory unknown (None) -> "mps".
        self.assertEqual(
            choose_device(
                cuda_available=False, free_vram_gb=None, mps_available=True,
                override="", free_unified_gb=None,
            ),
            "mps",
        )
        # CUDA available but too little VRAM, MPS present, low unified -> "cpu".
        self.assertEqual(
            choose_device(
                cuda_available=True, free_vram_gb=threshold - 1.0, mps_available=True,
                override="", free_unified_gb=threshold - 1.0,
            ),
            "cpu",
        )
        # CUDA available but too little VRAM, MPS present, sufficient unified -> "mps".
        self.assertEqual(
            choose_device(
                cuda_available=True, free_vram_gb=threshold - 1.0, mps_available=True,
                override="", free_unified_gb=threshold + 1.0,
            ),
            "mps",
        )
        # Exactly AT the floor is sufficient (the gate is strict `<`) -> "mps".
        self.assertEqual(
            choose_device(
                cuda_available=False, free_vram_gb=None, mps_available=True,
                override="", free_unified_gb=threshold,
            ),
            "mps",
        )
        # CUDA with enough VRAM wins BEFORE the MPS gate; low unified is irrelevant.
        self.assertEqual(
            choose_device(
                cuda_available=True, free_vram_gb=threshold + 1.0, mps_available=True,
                override="", free_unified_gb=threshold - 1.0,
            ),
            "cuda",
        )

    def test_embed_texts_recovers_from_cuda_oom_by_retrying_on_cpu(self):
        # A CUDA OOM during encode must NOT crash the index: the embedder frees
        # VRAM, forces CPU for the rest of the process, and retries the batch on
        # CPU so indexing continues (degraded) instead of hanging/dying.
        import oracle.ingestion.embedder as embedder

        class _FakeModel:
            def __init__(self):
                self.calls = []

            def encode(self, texts, batch_size=4, show_progress_bar=False):
                self.calls.append(list(texts))
                if len(self.calls) == 1:
                    raise RuntimeError("CUDA out of memory. Tried to allocate ...")
                return [[0.1, 0.2, 0.3] for _ in texts]

        fake = _FakeModel()
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}, clear=False), patch.object(
            embedder, "_sentence_model", return_value=fake
        ), patch.object(embedder, "embedding_device", return_value="cuda"), patch.object(
            embedder, "release_embedding_memory"
        ) as release, patch.object(embedder, "_force_cpu_after_oom") as force_cpu:
            vectors = embed_texts(["hello world"], require_sentence_transformer=True)

        self.assertEqual(len(vectors), 1)
        self.assertEqual(len(vectors[0]), 3)
        # Encoded twice: once (OOM) on cuda, once (success) on the cpu retry.
        self.assertEqual(len(fake.calls), 2)
        release.assert_called()
        force_cpu.assert_called()


class OracleServerMainBindTest(unittest.TestCase):
    """Double-spawn fix (secondary): the resident-server entrypoint must bind the
    fixed session port AS EARLY AS POSSIBLE (in __main__, before the heavy FastAPI/
    routes imports) so a DUPLICATE spawn collides on the bind and `os._exit(1)`s in
    milliseconds instead of lingering in import startup as an untracked zombie.

    Importing the module must NOT bind any socket (only __main__ binds); the `app`
    attribute must still be importable, built lazily on first access without binding.
    """

    def test_importing_main_does_not_bind_a_socket(self):
        import importlib
        import socket

        # Drop any cached import so we observe a fresh module load.
        import sys

        sys.modules.pop("oracle.server.main", None)

        original_bind = socket.socket.bind
        bind_calls = []

        def recording_bind(self, address):  # noqa: ANN001
            bind_calls.append(address)
            return original_bind(self, address)

        try:
            socket.socket.bind = recording_bind
            importlib.import_module("oracle.server.main")
        finally:
            socket.socket.bind = original_bind

        self.assertEqual(
            bind_calls,
            [],
            "importing oracle.server.main must NOT bind a socket (only __main__ binds)",
        )

    def test_app_is_importable_and_built_lazily_without_binding(self):
        import importlib
        import socket
        import sys

        sys.modules.pop("oracle.server.main", None)

        original_bind = socket.socket.bind
        bind_calls = []

        def recording_bind(self, address):  # noqa: ANN001
            bind_calls.append(address)
            return original_bind(self, address)

        try:
            socket.socket.bind = recording_bind
            main = importlib.import_module("oracle.server.main")
            # First access to `app` builds the FastAPI app lazily (PEP 562 __getattr__)
            # — still WITHOUT binding any socket.
            app = main.app
            app_again = main.build_app()
        finally:
            socket.socket.bind = original_bind

        self.assertEqual(type(app).__name__, "FastAPI")
        self.assertIs(app, app_again, "build_app must cache and return one instance")
        self.assertEqual(
            bind_calls,
            [],
            "building the app must not bind a socket",
        )

    def test_bind_listen_socket_returns_a_listening_socket_on_a_free_port(self):
        import importlib
        import socket

        main = importlib.import_module("oracle.server.main")

        # Pick a likely-free ephemeral port by binding+closing a probe socket.
        probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        probe.bind(("127.0.0.1", 0))
        free_port = probe.getsockname()[1]
        probe.close()

        sock = main._bind_listen_socket("127.0.0.1", free_port)
        try:
            self.assertEqual(sock.getsockname()[1], free_port)
            # It is actually listening (a second bind to the same port must fail).
            clash = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            with self.assertRaises(OSError):
                clash.bind(("127.0.0.1", free_port))
            clash.close()
        finally:
            sock.close()


class OracleVendoredEnvExclusionTest(unittest.TestCase):
    """A vendored Python environment / installed-package tree (site-packages,
    pip `*.dist-info`/`*.egg-info` install markers, RECORD+WHEEL/METADATA) is
    bundled DATA, not workspace source. Oracle must never walk/index a
    workspace's bundled libraries + ML artifacts (e.g. aspis-biovision/Orasis/),
    which would balloon the index by ~15k library `.py`. Detection is by
    SIGNATURE so it generalizes beyond a name list; real source `.py` is never
    dropped, and the secret default-deny is unaffected.
    """

    def test_is_vendored_env_path_site_packages_component(self):
        self.assertTrue(
            is_vendored_env_path("Orasis/Lib/site-packages/numpy/core/_methods.py")
        )

    def test_is_vendored_env_path_dist_info_component(self):
        self.assertTrue(
            is_vendored_env_path("Orasis/numpy-1.26.4.dist-info/RECORD")
        )

    def test_is_vendored_env_path_egg_info_component(self):
        self.assertTrue(
            is_vendored_env_path("vendor/somepkg-2.0.egg-info/PKG-INFO")
        )

    def test_is_vendored_env_path_keeps_normal_source(self):
        self.assertFalse(is_vendored_env_path("src/app.py"))
        self.assertFalse(is_vendored_env_path("aspis-biovision/src/pipeline.py"))
        # A package merely NAMED like a dist must not trip the string rule: only a
        # real `*.dist-info`/`*.egg-info` directory component counts.
        self.assertFalse(is_vendored_env_path("src/dist_info_helpers/util.py"))

    def test_directory_contains_install_marker_dist_info(self):
        with tempfile.TemporaryDirectory() as tmp:
            env_root = Path(tmp) / "Orasis"
            (env_root / "numpy-1.26.4.dist-info").mkdir(parents=True)
            (env_root / "numpy").mkdir()
            self.assertTrue(directory_contains_install_marker(env_root))

    def test_directory_contains_install_marker_record_and_wheel(self):
        with tempfile.TemporaryDirectory() as tmp:
            env_root = Path(tmp) / "vendored"
            env_root.mkdir(parents=True)
            (env_root / "RECORD").write_text("numpy/__init__.py,,\n", encoding="utf-8")
            (env_root / "WHEEL").write_text("Wheel-Version: 1.0\n", encoding="utf-8")
            self.assertTrue(directory_contains_install_marker(env_root))

    def test_directory_contains_install_marker_false_on_source_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "src"
            src.mkdir(parents=True)
            (src / "app.py").write_text("print('hi')\n", encoding="utf-8")
            self.assertFalse(directory_contains_install_marker(src))

    def test_collect_text_files_skips_vendored_env_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            # A vendored env: a package dir sits next to its pip install marker.
            # The marker dir has NO indexable text of its own beyond RECORD, but
            # the sibling package's library `.py` must be pruned too because the
            # whole tree under the install-marker root is bundled, not source.
            dist_info = root / "Orasis" / "numpy-1.26.4.dist-info"
            dist_info.mkdir(parents=True)
            (dist_info / "RECORD").write_text("numpy/__init__.py,,\n", encoding="utf-8")
            (dist_info / "METADATA").write_text("Name: numpy\n", encoding="utf-8")
            lib_pkg = root / "Orasis" / "numpy"
            lib_pkg.mkdir(parents=True)
            (lib_pkg / "__init__.py").write_text("__version__ = '1.26.4'\n", encoding="utf-8")
            (lib_pkg / "core.py").write_text("def add(a, b):\n    return a + b\n", encoding="utf-8")
            # A site-packages tree elsewhere.
            sp = root / "build" / "Lib" / "site-packages" / "torch"
            sp.mkdir(parents=True)
            (sp / "_tensor.py").write_text("class Tensor:\n    pass\n", encoding="utf-8")
            # Real workspace source that must survive.
            real = root / "aspis-biovision" / "src"
            real.mkdir(parents=True)
            (real / "pipeline.py").write_text("def run():\n    return 1\n", encoding="utf-8")

            collected = {path.relative_to(root).as_posix() for path in collect_text_files(root)}
            self.assertIn("aspis-biovision/src/pipeline.py", collected)
            self.assertNotIn("Orasis/numpy/__init__.py", collected)
            self.assertNotIn("Orasis/numpy/core.py", collected)
            self.assertNotIn("Orasis/numpy-1.26.4.dist-info/RECORD", collected)
            self.assertNotIn("build/Lib/site-packages/torch/_tensor.py", collected)

    def test_vendored_exclusion_does_not_regress_secret_default_deny(self):
        # The secret default-deny must keep firing independently of the vendored
        # filter: a token.txt outside any env is still dropped, and source is kept.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            secret = root / "config" / "token.txt"
            secret.parent.mkdir(parents=True)
            secret.write_text("ghp_deadbeef\n", encoding="utf-8")
            src = root / "src" / "worker.ts"
            src.parent.mkdir(parents=True)
            src.write_text("export const x = 1;\n", encoding="utf-8")
            collected = {path.relative_to(root).as_posix() for path in collect_text_files(root)}
            self.assertIn("src/worker.ts", collected)
            self.assertNotIn("config/token.txt", collected)


if __name__ == "__main__":
    unittest.main()
