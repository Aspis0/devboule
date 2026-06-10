"""Tests for Step 4a: the MCP thin-client + bounded/scoped HTTP endpoints.

The MCP process talks to a resident HTTP Oracle server when one is published
(env override or discovery file), and falls back to the in-process QueryEngine
on any failure or when no target resolves. The thin-client must never load the
embedder when the HTTP path is used, must pass the locally-computed
`allowed_file_ids` scope (the server never widens it), and must never log the
auth token or an absolute path.
"""

import io
import json
import logging
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import oracle.config as oracle_config
from oracle.server.aspis_mcp import (
    HttpOracleEngine,
    handle_tool_call,
    resolve_oracle_http_target,
)
from oracle.store.sqlite_store import SQLiteStore


def prepare_management_root(root: Path) -> Path:
    (root / "config.json").write_text("{}", encoding="utf-8")
    (root / "oracle" / "server").mkdir(parents=True, exist_ok=True)
    (root / "oracle" / "server" / "aspis_mcp.py").write_text("# test marker\n", encoding="utf-8")
    projects = root / "projects"
    projects.mkdir(exist_ok=True)
    return projects


def write_discovery_file(projects_dir: Path, base_url: str, token: str) -> Path:
    payload = {
        "baseUrl": base_url,
        "authToken": token,
        "indexRoot": str(projects_dir.parent),
        "pid": 1234,
        "updatedAt": "2026-06-01T00:00:00Z",
    }
    path = projects_dir / ".oracle-server.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


class ResolveHttpTargetTests(unittest.TestCase):
    def setUp(self):
        self._saved = {
            key: os.environ.get(key)
            for key in ("ASPIS_ORACLE_HTTP_BASE", "ASPIS_ORACLE_AUTH_TOKEN")
        }
        for key in self._saved:
            os.environ.pop(key, None)

    def tearDown(self):
        for key, value in self._saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value

    def test_env_override_wins_over_discovery_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            write_discovery_file(projects, "http://127.0.0.1:9999", "file-token")
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            target = resolve_oracle_http_target(projects)
        self.assertEqual(target, ("http://127.0.0.1:8765", "env-token"))

    def test_env_override_requires_both_vars(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            # No token -> env override is incomplete; with no file -> None.
            target = resolve_oracle_http_target(projects)
        self.assertIsNone(target)

    def test_discovery_file_parsed_when_env_absent(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            write_discovery_file(projects, "http://127.0.0.1:7000", "file-token")
            target = resolve_oracle_http_target(projects)
        self.assertEqual(target, ("http://127.0.0.1:7000", "file-token"))

    def test_missing_file_returns_none(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            self.assertIsNone(resolve_oracle_http_target(projects))

    def test_corrupt_file_returns_none(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            (projects / ".oracle-server.json").write_text("{not json", encoding="utf-8")
            self.assertIsNone(resolve_oracle_http_target(projects))

    def test_partial_file_returns_none(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            # baseUrl present but authToken missing.
            (projects / ".oracle-server.json").write_text(
                json.dumps({"baseUrl": "http://127.0.0.1:7000"}), encoding="utf-8"
            )
            self.assertIsNone(resolve_oracle_http_target(projects))

    # FIX 2: loopback enforcement on the resolved base_url.
    def test_remote_base_url_rejected(self):
        from oracle.server.aspis_mcp import _reset_oracle_target_cache

        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            write_discovery_file(projects, "https://evil.example/oracle", "file-token")
            _reset_oracle_target_cache()
            self.assertIsNone(resolve_oracle_http_target(projects))

    def test_localhost_base_url_accepted(self):
        from oracle.server.aspis_mcp import _reset_oracle_target_cache

        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            write_discovery_file(projects, "http://localhost:7000", "file-token")
            _reset_oracle_target_cache()
            self.assertEqual(
                resolve_oracle_http_target(projects), ("http://localhost:7000", "file-token")
            )

    def test_loopback_ipv4_base_url_accepted(self):
        from oracle.server.aspis_mcp import _reset_oracle_target_cache

        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            write_discovery_file(projects, "http://127.0.0.1:7000", "file-token")
            _reset_oracle_target_cache()
            self.assertEqual(
                resolve_oracle_http_target(projects), ("http://127.0.0.1:7000", "file-token")
            )

    def test_env_override_remote_base_url_rejected(self):
        from oracle.server.aspis_mcp import _reset_oracle_target_cache

        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "https://evil.example/oracle"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            _reset_oracle_target_cache()
            self.assertIsNone(resolve_oracle_http_target(projects))

    # FIX 8: short TTL cache keyed on projects_dir.
    def test_target_is_ttl_cached(self):
        from oracle.server.aspis_mcp import _reset_oracle_target_cache

        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            write_discovery_file(projects, "http://127.0.0.1:7000", "file-token")
            _reset_oracle_target_cache()
            with patch(
                "oracle.server.aspis_mcp._read_oracle_discovery_file",
                wraps=__import__("oracle.server.aspis_mcp", fromlist=["_read_oracle_discovery_file"])._read_oracle_discovery_file,
            ) as reader:
                resolve_oracle_http_target(projects)
                resolve_oracle_http_target(projects)
                self.assertEqual(reader.call_count, 1)


class _Recorder:
    """Captures the last POST body so tests can assert on the scope."""

    def __init__(self, response_json, raise_exc=None, status_code=200):
        self.response_json = response_json
        self.raise_exc = raise_exc
        self.status_code = status_code
        self.calls = []

    def client(self, *args, **kwargs):
        recorder = self

        class _Client:
            def __enter__(self_inner):
                return self_inner

            def __exit__(self_inner, *exc):
                return False

            def post(self_inner, url, headers=None, json=None):
                recorder.calls.append({"url": url, "headers": headers, "json": json})
                if recorder.raise_exc is not None:
                    raise recorder.raise_exc

                class _Resp:
                    status_code = recorder.status_code
                    content = b"x"
                    request = SimpleNamespace(url=url)

                    def raise_for_status(self_resp):
                        if recorder.status_code >= 400:
                            raise RuntimeError(f"HTTP {recorder.status_code}")

                    def json(self_resp):
                        return recorder.response_json

                return _Resp()

        return _Client()


class ThinClientRoutingTests(unittest.TestCase):
    def setUp(self):
        self._saved = {
            key: os.environ.get(key)
            for key in (
                "ASPIS_ORACLE_HTTP_BASE",
                "ASPIS_ORACLE_AUTH_TOKEN",
                "ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS",
                "ASPIS_MCP_DISABLE_APP_VAULT",
            )
        }
        for key in ("ASPIS_ORACLE_HTTP_BASE", "ASPIS_ORACLE_AUTH_TOKEN"):
            os.environ.pop(key, None)
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = "1"
        from oracle.server.aspis_mcp import _reset_oracle_target_cache

        _reset_oracle_target_cache()

    def tearDown(self):
        for key, value in self._saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value

    def _register(self, root: Path):
        handle_tool_call(
            "agent_register",
            {"agent_id": "thin-agent", "role": "orchestrator", "model": "test", "message": "go"},
            root=root,
        )

    def test_context_uses_http_path_and_passes_scope_without_embedder(self):
        recorder = _Recorder({"query": "q", "chunks": [{"chunk_id": "c1"}]})
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                with patch("oracle.server.aspis_mcp.ensure_oracle_index_ready", return_value={"root": str(root), "indexed_files": 1, "pending_files": 0, "stale_files": 0}):
                    with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py", "b.py"}):
                        with patch("oracle.server.aspis_mcp.make_mcp_engine", side_effect=AssertionError("in-process engine must not be built on HTTP path")):
                            result = handle_tool_call(
                                "oracle_context",
                                {"query": "where", "limit": 4, "agent_id": "thin-agent", "role": "orchestrator"},
                                root=root,
                            )
        self.assertEqual(result["chunks"], [{"chunk_id": "c1"}])
        self.assertEqual(len(recorder.calls), 1)
        call = recorder.calls[0]
        self.assertTrue(call["url"].endswith("/context-bounded"))
        self.assertEqual(sorted(call["json"]["allowed_file_ids"]), ["a.py", "b.py"])
        self.assertEqual(call["json"]["limit"], 4)
        self.assertEqual(call["headers"].get("x-oracle-auth-token"), "env-token")

    def test_ask_uses_http_path_and_passes_scope(self):
        recorder = _Recorder({"answer": "from-http", "citations": [], "not_found": False, "results": []})
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                with patch("oracle.server.aspis_mcp.ensure_oracle_index_ready", return_value={"root": str(root), "indexed_files": 1, "pending_files": 0, "stale_files": 0}):
                    with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"only.py"}):
                        with patch("oracle.server.aspis_mcp.make_mcp_engine", side_effect=AssertionError("in-process engine must not be built on HTTP path")):
                            result = handle_tool_call(
                                "oracle_ask",
                                {"query": "what", "limit": 3, "agent_id": "thin-agent", "role": "orchestrator"},
                                root=root,
                            )
        self.assertEqual(result["answer"], "from-http")
        call = recorder.calls[0]
        self.assertTrue(call["url"].endswith("/ask-bounded"))
        self.assertEqual(call["json"]["allowed_file_ids"], ["only.py"])

    def test_http_failure_falls_back_to_in_process(self):
        recorder = _Recorder(None, raise_exc=ConnectionError("refused"))

        class FakeEngine:
            def __init__(self):
                self.context_called = False

            def context(self, query, limit=8, allowed_file_ids=None):
                self.context_called = True
                return [{"chunk_id": "local-1", "file_source": "a.py"}]

        fake = FakeEngine()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                with patch("oracle.server.aspis_mcp.ensure_oracle_index_ready", return_value={"root": str(root), "indexed_files": 1, "pending_files": 0, "stale_files": 0}):
                    with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                        with patch("oracle.server.aspis_mcp.make_mcp_engine", return_value=fake):
                            with patch.dict(os.environ, {"ASPIS_MCP_DENSE_CONTEXT": "1"}):
                                result = handle_tool_call(
                                    "oracle_context",
                                    {"query": "where", "limit": 2, "agent_id": "thin-agent", "role": "orchestrator"},
                                    root=root,
                                )
        self.assertTrue(fake.context_called)
        self.assertEqual(result["chunks"][0]["chunk_id"], "local-1")

    def test_no_target_uses_in_process(self):
        class FakeEngine:
            def __init__(self):
                self.context_called = False

            def context(self, query, limit=8, allowed_file_ids=None):
                self.context_called = True
                return [{"chunk_id": "local-only", "file_source": "a.py"}]

        fake = FakeEngine()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            # No env override, no discovery file.
            with patch("oracle.server.aspis_mcp.import_httpx", side_effect=AssertionError("HTTP must not be used with no target")):
                with patch("oracle.server.aspis_mcp.ensure_oracle_index_ready", return_value={"root": str(root), "indexed_files": 1, "pending_files": 0, "stale_files": 0}):
                    with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                        with patch("oracle.server.aspis_mcp.make_mcp_engine", return_value=fake):
                            with patch.dict(os.environ, {"ASPIS_MCP_DENSE_CONTEXT": "1"}):
                                result = handle_tool_call(
                                    "oracle_context",
                                    {"query": "where", "limit": 2, "agent_id": "thin-agent", "role": "orchestrator"},
                                    root=root,
                                )
        self.assertTrue(fake.context_called)
        self.assertEqual(result["chunks"][0]["chunk_id"], "local-only")

    def test_thin_client_logger_never_emits_token_or_absolute_path(self):
        recorder = _Recorder(None, raise_exc=ConnectionError("connection refused"))
        stream = io.StringIO()
        handler = logging.StreamHandler(stream)
        logger = logging.getLogger("oracle.server.aspis_mcp")
        logger.addHandler(handler)
        old_level = logger.level
        logger.setLevel(logging.DEBUG)

        class FakeEngine:
            def context(self, query, limit=8, allowed_file_ids=None):
                return [{"chunk_id": "local", "file_source": "a.py"}]

        try:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                prepare_management_root(root)
                self._register(root)
                os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
                os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "super-secret-token-value"
                with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                    with patch("oracle.server.aspis_mcp.ensure_oracle_index_ready", return_value={"root": str(root), "indexed_files": 1, "pending_files": 0, "stale_files": 0}):
                        with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                            with patch("oracle.server.aspis_mcp.make_mcp_engine", return_value=FakeEngine()):
                                with patch.dict(os.environ, {"ASPIS_MCP_DENSE_CONTEXT": "1"}):
                                    handle_tool_call(
                                        "oracle_context",
                                        {"query": "where", "limit": 2, "agent_id": "thin-agent", "role": "orchestrator"},
                                        root=root,
                                    )
        finally:
            logger.removeHandler(handler)
            logger.setLevel(old_level)
        logs = stream.getvalue()
        self.assertNotIn("super-secret-token-value", logs)
        self.assertNotIn("C:\\", logs)
        self.assertNotIn("/home/", logs)


class BoundedEndpointTests(unittest.TestCase):
    def setUp(self):
        from fastapi import FastAPI
        from fastapi.testclient import TestClient

        self._tmp = tempfile.TemporaryDirectory()
        root = Path(self._tmp.name)
        sqlite_path = root / "metadata.sqlite"
        sqlite = SQLiteStore(sqlite_path)
        chunks = [
            {
                "id": "alpha.py#chunk-0000",
                "file_id": "alpha.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 40,
                "text": "alpha module defines the widget pipeline",
                "file_sorgente": "alpha.py",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            },
            {
                "id": "beta.py#chunk-0000",
                "file_id": "beta.py",
                "chunk_index": 0,
                "start_char": 0,
                "end_char": 40,
                "text": "beta module defines the widget renderer",
                "file_sorgente": "beta.py",
                "ultima_modifica": "2026-05-28T00:00:00Z",
                "embedding_dims": 1024,
            },
        ]
        sqlite.replace_chunks_for_files(["alpha.py", "beta.py"], chunks)

        self._old = (
            oracle_config.SQLITE_PATH,
            oracle_config.LANCE_DB_PATH,
            oracle_config.CHUNK_DB_PATH,
            oracle_config.ORACLE_AUTH_TOKEN,
        )
        oracle_config.SQLITE_PATH = sqlite_path
        oracle_config.LANCE_DB_PATH = root / "vectors.json"
        oracle_config.CHUNK_DB_PATH = root / "chunks.json"
        oracle_config.ORACLE_AUTH_TOKEN = "test-auth-token"

        from oracle.server.routes import create_router

        app = FastAPI()
        app.include_router(create_router())
        self.client = TestClient(app)
        self.headers = {"x-oracle-auth-token": "test-auth-token"}

    def tearDown(self):
        (
            oracle_config.SQLITE_PATH,
            oracle_config.LANCE_DB_PATH,
            oracle_config.CHUNK_DB_PATH,
            oracle_config.ORACLE_AUTH_TOKEN,
        ) = self._old
        self._tmp.cleanup()

    def test_context_bounded_requires_auth(self):
        resp = self.client.post("/context-bounded", json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]})
        self.assertEqual(resp.status_code, 401)

    def test_ask_bounded_requires_auth(self):
        resp = self.client.post("/ask-bounded", json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]})
        self.assertEqual(resp.status_code, 401)

    def test_context_bounded_empty_scope_returns_no_documents(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.headers,
            json={"query": "widget", "limit": 5, "allowed_file_ids": []},
        )
        self.assertEqual(resp.status_code, 200)
        self.assertEqual(resp.json()["chunks"], [])

    def test_context_bounded_constrains_to_allowed_ids(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.headers,
            json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp.status_code, 200)
        chunks = resp.json()["chunks"]
        self.assertTrue(chunks)
        for chunk in chunks:
            self.assertEqual(chunk["file_source"], "alpha.py")

    def test_ask_bounded_empty_scope_is_grounded_empty(self):
        with patch("oracle.server.answerer.generate_with_openai_compatible") as generate:
            resp = self.client.post(
                "/ask-bounded",
                headers=self.headers,
                json={"query": "widget", "limit": 5, "allowed_file_ids": []},
            )
        self.assertEqual(resp.status_code, 200)
        body = resp.json()
        self.assertEqual(body["results"], [])
        self.assertFalse(generate.called)

    # FIX 9: parse_bounded_payload input validation.
    def test_negative_limit_clamped_to_one(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.headers,
            json={"query": "widget", "limit": -5, "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp.status_code, 200)

    def test_huge_limit_clamped(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.headers,
            json={"query": "widget", "limit": 100000, "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp.status_code, 200)

    def test_non_int_limit_returns_422(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.headers,
            json={"query": "widget", "limit": "abc", "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp.status_code, 422)

    def test_oversized_id_list_returns_422(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.headers,
            json={"query": "widget", "limit": 5, "allowed_file_ids": [f"f{i}.py" for i in range(10001)]},
        )
        self.assertEqual(resp.status_code, 422)

    def test_wrong_type_ids_returns_422(self):
        for bad in ({"a": 1}, 5, "alpha.py"):
            resp = self.client.post(
                "/context-bounded",
                headers=self.headers,
                json={"query": "widget", "limit": 5, "allowed_file_ids": bad},
            )
            self.assertEqual(resp.status_code, 422, f"ids={bad!r}")

    def test_null_ids_is_empty_scope(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.headers,
            json={"query": "widget", "limit": 5, "allowed_file_ids": None},
        )
        self.assertEqual(resp.status_code, 200)
        self.assertEqual(resp.json()["chunks"], [])

    def test_absent_ids_is_empty_scope(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.headers,
            json={"query": "widget", "limit": 5},
        )
        self.assertEqual(resp.status_code, 200)
        self.assertEqual(resp.json()["chunks"], [])

    def test_ask_bounded_constrains_to_allowed_ids(self):
        with patch(
            "oracle.server.answerer.generate_with_openai_compatible",
            return_value=json.dumps(
                {"answer": "alpha defines the widget pipeline", "citations": [{"ref": "C1"}], "not_found": False, "suggested_path": None}
            ),
        ):
            resp = self.client.post(
                "/ask-bounded",
                headers=self.headers,
                json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]},
            )
        self.assertEqual(resp.status_code, 200)
        body = resp.json()
        for result in body["results"]:
            self.assertIn("alpha.py", result["id"])

    # FIX 1 (privacy): the operator POST /context keeps the query OUT of the URL
    # (uvicorn access logs / proxies) while serving the SAME full-corpus retrieval
    # as GET /context — NOT the scoped/empty semantics of /context-bounded.
    def test_post_context_requires_auth(self):
        resp = self.client.post("/context", json={"q": "widget", "limit": 5})
        self.assertEqual(resp.status_code, 401)

    def test_post_context_returns_full_corpus_chunks_like_get(self):
        # Body-only query (no `q` URL param). Full corpus, NOT an empty scope: the
        # indexed `widget` chunks come back, mirroring GET /context's shape.
        resp = self.client.post(
            "/context",
            headers=self.headers,
            json={"q": "widget", "limit": 5},
        )
        self.assertEqual(resp.status_code, 200)
        body = resp.json()
        self.assertEqual(body["query"], "widget")
        chunks = body["chunks"]
        self.assertTrue(chunks, "POST /context must serve the full corpus, not an empty scope")
        sources = {chunk["file_source"] for chunk in chunks}
        self.assertEqual(sources, {"alpha.py", "beta.py"})
        # Mirrors the GET /context response shape exactly (same envelope keys).
        get_resp = self.client.get("/context", headers=self.headers, params={"q": "widget", "limit": 5})
        self.assertEqual(get_resp.json().keys(), body.keys())
        self.assertEqual(
            {chunk["file_source"] for chunk in get_resp.json()["chunks"]}, sources
        )

    def test_post_context_accepts_query_alias_and_defaults_limit(self):
        # `query` is accepted as an alias for `q` (mirrors ask_post); an absent
        # limit falls back to the default rather than 422-ing.
        resp = self.client.post(
            "/context",
            headers=self.headers,
            json={"query": "widget"},
        )
        self.assertEqual(resp.status_code, 200)
        self.assertEqual(resp.json()["query"], "widget")

    def test_post_context_non_int_limit_falls_back(self):
        # A non-int limit degrades to the default (ask_post semantics), never 422.
        resp = self.client.post(
            "/context",
            headers=self.headers,
            json={"q": "widget", "limit": "abc"},
        )
        self.assertEqual(resp.status_code, 200)

    def test_post_context_clamps_out_of_range_limit(self):
        # A negative/zero limit is clamped to 1 (it must never reach the engine
        # raw, and never 500); a huge limit is clamped to MAX_BOUNDED_LIMIT —
        # parse_bounded_payload semantics on the operator mirror.
        for bad_limit in (-5, 0):
            resp = self.client.post(
                "/context",
                headers=self.headers,
                json={"q": "widget", "limit": bad_limit},
            )
            self.assertEqual(resp.status_code, 200)
            # The engine may merge dense+lexical results, so the cap is not an
            # exact chunk count — the contract is "clamped, never a 500/raw pass".
            self.assertTrue(resp.json()["chunks"])
        resp = self.client.post(
            "/context",
            headers=self.headers,
            json={"q": "widget", "limit": 10_000},
        )
        self.assertEqual(resp.status_code, 200)
        # Clamped to MAX_BOUNDED_LIMIT (100) — the tiny corpus just returns all of it.
        self.assertLessEqual(len(resp.json()["chunks"]), 100)


class TwoTierAuthTests(unittest.TestCase):
    """FIX 1: operator token authorizes everything; agent token only /*-bounded."""

    def setUp(self):
        from fastapi import FastAPI
        from fastapi.testclient import TestClient

        self._tmp = tempfile.TemporaryDirectory()
        root = Path(self._tmp.name)
        sqlite_path = root / "metadata.sqlite"
        sqlite = SQLiteStore(sqlite_path)
        sqlite.replace_chunks_for_files(
            ["alpha.py"],
            [
                {
                    "id": "alpha.py#chunk-0000",
                    "file_id": "alpha.py",
                    "chunk_index": 0,
                    "start_char": 0,
                    "end_char": 40,
                    "text": "alpha module defines the widget pipeline",
                    "file_sorgente": "alpha.py",
                    "ultima_modifica": "2026-05-28T00:00:00Z",
                    "embedding_dims": 1024,
                }
            ],
        )
        self._old = (
            oracle_config.SQLITE_PATH,
            oracle_config.LANCE_DB_PATH,
            oracle_config.CHUNK_DB_PATH,
            oracle_config.ORACLE_AUTH_TOKEN,
            oracle_config.ORACLE_AGENT_AUTH_TOKEN,
        )
        oracle_config.SQLITE_PATH = sqlite_path
        oracle_config.LANCE_DB_PATH = root / "vectors.json"
        oracle_config.CHUNK_DB_PATH = root / "chunks.json"
        oracle_config.ORACLE_AUTH_TOKEN = "operator-token"
        oracle_config.ORACLE_AGENT_AUTH_TOKEN = "agent-token"

        from oracle.server.routes import create_router

        app = FastAPI()
        app.include_router(create_router())
        self.client = TestClient(app)
        self.operator = {"x-oracle-auth-token": "operator-token"}
        self.agent = {"x-oracle-auth-token": "agent-token"}

    def tearDown(self):
        (
            oracle_config.SQLITE_PATH,
            oracle_config.LANCE_DB_PATH,
            oracle_config.CHUNK_DB_PATH,
            oracle_config.ORACLE_AUTH_TOKEN,
            oracle_config.ORACLE_AGENT_AUTH_TOKEN,
        ) = self._old
        self._tmp.cleanup()

    # --- agent token: only bounded endpoints ---
    def test_agent_token_allows_context_bounded(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.agent,
            json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp.status_code, 200)

    def test_agent_token_allows_ask_bounded(self):
        with patch("oracle.server.answerer.generate_with_openai_compatible") as generate:
            resp = self.client.post(
                "/ask-bounded",
                headers=self.agent,
                json={"query": "widget", "limit": 5, "allowed_file_ids": []},
            )
        self.assertEqual(resp.status_code, 200)
        self.assertFalse(generate.called)

    def test_agent_token_rejected_on_unscoped_ask_get(self):
        resp = self.client.get("/ask", headers=self.agent, params={"q": "widget"})
        self.assertEqual(resp.status_code, 401)

    def test_agent_token_rejected_on_unscoped_context(self):
        resp = self.client.get("/context", headers=self.agent, params={"q": "widget"})
        self.assertEqual(resp.status_code, 401)

    def test_agent_token_rejected_on_ask_post(self):
        resp = self.client.post("/ask", headers=self.agent, json={"query": "widget"})
        self.assertEqual(resp.status_code, 401)

    def test_agent_token_rejected_on_post_context(self):
        # FIX 1: POST /context is an OPERATOR endpoint (the app's card-localization
        # path), never an agent/MCP one — an agent token must be rejected exactly
        # like the unscoped GET /context.
        resp = self.client.post("/context", headers=self.agent, json={"q": "widget"})
        self.assertEqual(resp.status_code, 401)

    def test_agent_token_rejected_on_index_run(self):
        resp = self.client.post("/index/run", headers=self.agent)
        self.assertEqual(resp.status_code, 401)

    # --- operator token: everything ---
    def test_operator_token_allows_context_bounded(self):
        resp = self.client.post(
            "/context-bounded",
            headers=self.operator,
            json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp.status_code, 200)

    def test_operator_token_allows_unscoped_context(self):
        resp = self.client.get("/context", headers=self.operator, params={"q": "widget"})
        self.assertEqual(resp.status_code, 200)

    def test_operator_token_allows_post_context(self):
        resp = self.client.post("/context", headers=self.operator, json={"q": "widget"})
        self.assertEqual(resp.status_code, 200)

    # --- no token: everything 401 ---
    def test_no_token_rejected_on_post_context(self):
        resp = self.client.post("/context", json={"q": "widget"})
        self.assertEqual(resp.status_code, 401)

    def test_no_token_rejected_on_bounded(self):
        resp = self.client.post(
            "/context-bounded",
            json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp.status_code, 401)

    def test_no_token_rejected_on_unscoped(self):
        resp = self.client.get("/context", params={"q": "widget"})
        self.assertEqual(resp.status_code, 401)

    def test_agent_tier_unavailable_when_unset(self):
        # When ORACLE_AGENT_AUTH_TOKEN is unset, the agent tier is unavailable;
        # only the operator token works on the bounded routes (backward compat).
        oracle_config.ORACLE_AGENT_AUTH_TOKEN = ""
        resp = self.client.post(
            "/context-bounded",
            headers=self.agent,
            json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp.status_code, 401)
        resp_op = self.client.post(
            "/context-bounded",
            headers=self.operator,
            json={"query": "widget", "limit": 5, "allowed_file_ids": ["alpha.py"]},
        )
        self.assertEqual(resp_op.status_code, 200)


def _httpx_with_status(status_code: int):
    """Build a SimpleNamespace mimicking the httpx module whose Client.post
    returns a response that raises httpx.HTTPStatusError on raise_for_status."""
    import httpx

    class _Client:
        def __init__(self, *a, **k):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def post(self, url, headers=None, json=None):
            request = httpx.Request("POST", url)
            response = httpx.Response(status_code, request=request, content=b"{}")
            return response

    return SimpleNamespace(Client=_Client, HTTPStatusError=httpx.HTTPStatusError)


class HttpResponseValidationTests(unittest.TestCase):
    """FIX 3: non-dict HTTP responses raise OracleHttpError (fall back)."""

    def _engine_with_response(self, response_json):
        recorder = _Recorder(response_json)
        return recorder

    def test_ask_non_dict_response_raises_http_error(self):
        from oracle.server.aspis_mcp import OracleHttpError

        for bad in (None, [], "str", 5):
            recorder = _Recorder(bad)
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
                with self.assertRaises(OracleHttpError):
                    engine.ask("q", 5, allowed_file_ids={"a.py"})

    def test_context_non_dict_response_raises_http_error(self):
        from oracle.server.aspis_mcp import OracleHttpError

        for bad in (None, "str", 5, {"chunks": "notalist"}):
            recorder = _Recorder(bad)
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
                with self.assertRaises(OracleHttpError):
                    engine.context("q", 5, allowed_file_ids={"a.py"})


class HttpStatusClassificationTests(unittest.TestCase):
    """FIX 5: 4xx surfaces (McpError); 5xx/connection -> OracleHttpError (fallback)."""

    def test_4xx_surfaces_as_mcp_error(self):
        from oracle.server.aspis_mcp import McpError

        engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=_httpx_with_status(401)):
            with self.assertRaises(McpError):
                engine.context("q", 5, allowed_file_ids={"a.py"})

    def test_422_surfaces_as_mcp_error(self):
        from oracle.server.aspis_mcp import McpError

        engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=_httpx_with_status(422)):
            with self.assertRaises(McpError):
                engine.ask("q", 5, allowed_file_ids={"a.py"})

    def test_5xx_falls_back_via_http_error(self):
        from oracle.server.aspis_mcp import OracleHttpError

        engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=_httpx_with_status(503)):
            with self.assertRaises(OracleHttpError):
                engine.context("q", 5, allowed_file_ids={"a.py"})

    def test_4xx_error_message_has_no_token_or_url(self):
        from oracle.server.aspis_mcp import McpError

        engine = HttpOracleEngine("http://127.0.0.1:8765", "tok-secret")
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=_httpx_with_status(403)):
            try:
                engine.context("q", 5, allowed_file_ids={"a.py"})
                self.fail("expected McpError")
            except McpError as exc:
                msg = str(exc)
                self.assertNotIn("tok-secret", msg)
                self.assertNotIn("127.0.0.1", msg)


class DispatchReadinessTests(unittest.TestCase):
    """FIX 4: HTTP path must NOT gate on the LOCAL readiness check."""

    def setUp(self):
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = "1"
        for key in ("ASPIS_ORACLE_HTTP_BASE", "ASPIS_ORACLE_AUTH_TOKEN"):
            os.environ.pop(key, None)
        from oracle.server.aspis_mcp import _reset_oracle_target_cache

        _reset_oracle_target_cache()

    def _register(self, root: Path):
        handle_tool_call(
            "agent_register",
            {"agent_id": "thin-agent", "role": "orchestrator", "model": "test", "message": "go"},
            root=root,
        )

    def test_empty_local_index_does_not_block_http_path(self):
        recorder = _Recorder({"query": "q", "chunks": [{"chunk_id": "c1"}]})
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            # ensure_oracle_index_ready would raise on an empty local index; it
            # must NOT be consulted on the HTTP path.
            from oracle.server.aspis_mcp import McpError

            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                with patch("oracle.server.aspis_mcp.ensure_oracle_index_ready", side_effect=McpError("Oracle index not ready")):
                    with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                        with patch("oracle.server.aspis_mcp.make_mcp_engine", side_effect=AssertionError("no in-process on HTTP path")):
                            result = handle_tool_call(
                                "oracle_context",
                                {"query": "where", "limit": 4, "agent_id": "thin-agent", "role": "orchestrator"},
                                root=root,
                            )
        self.assertEqual(result["chunks"], [{"chunk_id": "c1"}])

    def test_no_target_empty_index_still_blocks(self):
        from oracle.server.aspis_mcp import McpError

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            # No HTTP target -> in-process path -> local readiness gate fires.
            with patch("oracle.server.aspis_mcp.ensure_oracle_index_ready", side_effect=McpError("Oracle index not ready")):
                with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                    with self.assertRaises(McpError):
                        handle_tool_call(
                            "oracle_context",
                            {"query": "where", "limit": 4, "agent_id": "thin-agent", "role": "orchestrator"},
                            root=root,
                        )

    def test_http_fails_then_in_process_not_ready_surfaces(self):
        from oracle.server.aspis_mcp import McpError

        recorder = _Recorder(None, raise_exc=ConnectionError("refused"))
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                with patch("oracle.server.aspis_mcp.ensure_oracle_index_ready", side_effect=McpError("Oracle index not ready")):
                    with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                        with self.assertRaises(McpError):
                            handle_tool_call(
                                "oracle_context",
                                {"query": "where", "limit": 4, "agent_id": "thin-agent", "role": "orchestrator"},
                                root=root,
                            )


class NoneScopeGuardTests(unittest.TestCase):
    """FIX 6: None scope on the HTTP path must raise, not silently diverge."""

    def test_dispatch_context_none_scope_raises(self):
        from oracle.server.aspis_mcp import McpError, dispatch_oracle_context, _reset_oracle_target_cache

        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            write_discovery_file(projects, "http://127.0.0.1:7000", "file-token")
            _reset_oracle_target_cache()
            with self.assertRaises(McpError):
                dispatch_oracle_context(projects, "q", 5, None)

    def test_dispatch_ask_none_scope_raises(self):
        from oracle.server.aspis_mcp import McpError, dispatch_oracle_ask, _reset_oracle_target_cache

        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            write_discovery_file(projects, "http://127.0.0.1:7000", "file-token")
            _reset_oracle_target_cache()
            with self.assertRaises(McpError):
                dispatch_oracle_ask(projects, "q", 5, None)


class EmptyScopeShapeParityTests(unittest.TestCase):
    """FIX 7: HTTP empty-scope ask dict matches in-process empty-scope keys."""

    def test_http_empty_scope_ask_keyset_superset(self):
        from oracle.server.aspis_mcp import mcp_oracle_ask

        class _EmptyEngine:
            class sqlite:
                @staticmethod
                def all_chunks():
                    return []

        in_proc = mcp_oracle_ask(_EmptyEngine(), "q", 5, allowed_file_ids=set())
        engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
        http_empty = engine.ask("q", 5, allowed_file_ids=set())
        missing = set(in_proc.keys()) - set(http_empty.keys())
        self.assertEqual(missing, set(), f"HTTP empty-scope is missing keys: {missing}")


class HttpOracleEngineUnitTests(unittest.TestCase):
    def test_context_empty_scope_short_circuits_without_http(self):
        recorder = _Recorder({"query": "q", "chunks": [{"chunk_id": "x"}]})
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
            engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
            chunks = engine.context("q", 5, allowed_file_ids=set())
        self.assertEqual(chunks, [])
        self.assertEqual(recorder.calls, [])


if __name__ == "__main__":
    unittest.main()
