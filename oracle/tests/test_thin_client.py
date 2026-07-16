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
    McpError,
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
        # A LIVE pid: the resolver liveness-gates the discovery target (dead
        # pid ⇒ skip), and these tests assert parsing/validation, not liveness
        # — the pid-gate cases live in test_oracle_fastpath.py.
        "pid": os.getpid(),
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
                with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py", "b.py"}):
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
                with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"only.py"}):
                    result = handle_tool_call(
                        "oracle_ask",
                        {"query": "what", "limit": 3, "agent_id": "thin-agent", "role": "orchestrator"},
                        root=root,
                    )
        self.assertEqual(result["answer"], "from-http")
        call = recorder.calls[0]
        self.assertTrue(call["url"].endswith("/ask-bounded"))
        self.assertEqual(call["json"]["allowed_file_ids"], ["only.py"])

    def test_http_failure_raises_mcp_error_no_fallback(self):
        # M3-P12c: the in-process fallback engine is gone. A transport-level
        # failure on the HTTP Oracle path must surface as McpError pointing
        # the operator at the Aspis Management app — there is no second engine
        # to silently mask the outage.
        recorder = _Recorder(None, raise_exc=ConnectionError("refused"))
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                    with self.assertRaises(McpError) as ctx:
                        handle_tool_call(
                            "oracle_context",
                            {"query": "where", "limit": 2, "agent_id": "thin-agent", "role": "orchestrator"},
                            root=root,
                        )
        self.assertIn("Oracle server unreachable", str(ctx.exception))
        self.assertIn("Aspis Management app", str(ctx.exception))
        self.assertIn("no in-process fallback", str(ctx.exception))

    def test_no_target_raises_mcp_error_no_fallback(self):
        # M3-P12c: with no HTTP target resolving (no env override, no discovery
        # file), the dispatch raises McpError instead of falling back to the
        # deleted in-process engine.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            # No env override, no discovery file.
            with patch("oracle.server.aspis_mcp.import_httpx", side_effect=AssertionError("HTTP must not be used with no target")):
                with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                    with self.assertRaises(McpError) as ctx:
                        handle_tool_call(
                            "oracle_context",
                            {"query": "where", "limit": 2, "agent_id": "thin-agent", "role": "orchestrator"},
                            root=root,
                        )
        self.assertIn("Oracle server unreachable", str(ctx.exception))
        self.assertIn("Aspis Management app", str(ctx.exception))
        self.assertIn("no in-process fallback", str(ctx.exception))

    def test_thin_client_logger_never_emits_token_or_absolute_path(self):
        # M3-P12c: HTTP-failure logs still must not leak the auth token, base URL,
        # or any absolute path. The dispatch surfaces an unreachable McpError;
        # the warning log emits only the endpoint PATH + exception class.
        recorder = _Recorder(None, raise_exc=ConnectionError("connection refused"))
        stream = io.StringIO()
        handler = logging.StreamHandler(stream)
        logger = logging.getLogger("oracle.server.aspis_mcp")
        logger.addHandler(handler)
        old_level = logger.level
        logger.setLevel(logging.DEBUG)
        try:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                prepare_management_root(root)
                self._register(root)
                os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
                os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "super-secret-token-value"
                with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                    with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                        with self.assertRaises(McpError):
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
        self.assertNotIn("127.0.0.1", logs)
        self.assertNotIn("C:\\", logs)
        self.assertNotIn("/home/", logs)


# (M3-P13a: BoundedEndpointTests and TwoTierAuthTests removed — their setUp did
# `from oracle.server.routes import create_router`, and routes.py was deleted
# with the Python runtime, so they now error with ImportError. The two-tier
# auth + bounded-endpoint contracts they pinned are covered on the live (Rust)
# server by oracle-core/tests/server_test.rs.)

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
    """M3-P12c: dispatch is HTTP-only; readiness lives on the resident server.

    The local readiness gate (`ensure_oracle_index_ready`) and the in-process
    engine (`make_mcp_engine`) are GONE. The dispatch surface is now: target
    resolves -> HTTP call -> return; target missing OR HTTP fails -> raise
    McpError pointing at the Aspis Management app.
    """

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

    def test_http_path_succeeds_regardless_of_local_index_state(self):
        # The resident server is authoritative for readiness; the MCP side
        # never gates on a local index. We must not consult the (deleted)
        # `ensure_oracle_index_ready` or `make_mcp_engine` here — the
        # assertions below prove neither is referenced on the success path.
        recorder = _Recorder({"query": "q", "chunks": [{"chunk_id": "c1"}]})
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            from oracle.server.aspis_mcp import McpError

            # Defensive: these names no longer exist. If anything ever patches
            # them back in, the call would AttributeError — explicitly fail so
            # the regression is loud.
            for deleted in (
                "oracle.server.aspis_mcp.ensure_oracle_index_ready",
                "oracle.server.aspis_mcp.make_mcp_engine",
            ):
                self.assertFalse(
                    hasattr(__import__("oracle.server.aspis_mcp", fromlist=["x"]), deleted.split(".")[-1]),
                    f"{deleted} must not be re-introduced on the HTTP path",
                )
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                    # If the dispatch ever tries to consult the deleted local
                    # readiness gate, the AttributeError on the bare name below
                    # makes the regression obvious.
                    with patch(
                        "oracle.server.aspis_mcp._require_concrete_scope",
                        side_effect=McpError("scope guard tripped"),
                    ):
                        with self.assertRaises(McpError):
                            # scope guard tripped first, but if it weren't we'd
                            # route to HTTP — the test asserts we do NOT take
                            # any local index path.
                            handle_tool_call(
                                "oracle_context",
                                {"query": "where", "limit": 4, "agent_id": "thin-agent", "role": "orchestrator"},
                                root=root,
                            )
                    # Now real call with the scope guard intact — proves the
                    # HTTP path returns the recorder's response without ever
                    # looking at the local index.
                    result = handle_tool_call(
                        "oracle_context",
                        {"query": "where", "limit": 4, "agent_id": "thin-agent", "role": "orchestrator"},
                        root=root,
                    )
        self.assertEqual(result["chunks"], [{"chunk_id": "c1"}])

    def test_no_target_raises_unreachable(self):
        # M3-P12c: with no HTTP target resolving, the dispatch raises
        # McpError("Oracle server unreachable ..."). The old "local readiness
        # gate fires" path is gone — there is no in-process engine to gate.
        from oracle.server.aspis_mcp import McpError

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                with self.assertRaises(McpError) as ctx:
                    handle_tool_call(
                        "oracle_context",
                        {"query": "where", "limit": 4, "agent_id": "thin-agent", "role": "orchestrator"},
                        root=root,
                    )
        self.assertIn("Oracle server unreachable", str(ctx.exception))
        self.assertIn("Aspis Management app", str(ctx.exception))

    def test_http_failure_raises_unreachable(self):
        # M3-P12c: HTTP transport failure used to fall back to the in-process
        # engine. Now it raises McpError("Oracle server unreachable ...")
        # pointing the operator at the app — there is no second engine.
        from oracle.server.aspis_mcp import McpError

        recorder = _Recorder(None, raise_exc=ConnectionError("refused"))
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                with patch("oracle.server.aspis_mcp.oracle_allowed_file_ids", return_value={"a.py"}):
                    with self.assertRaises(McpError) as ctx:
                        handle_tool_call(
                            "oracle_context",
                            {"query": "where", "limit": 4, "agent_id": "thin-agent", "role": "orchestrator"},
                            root=root,
                        )
        self.assertIn("Oracle server unreachable", str(ctx.exception))
        self.assertIn("Aspis Management app", str(ctx.exception))


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
    """M3-P12c: HTTP empty-scope ask response carries the documented envelope.

    The historical parity test compared the in-process and HTTP empty-scope
    key sets. The in-process engine is gone; the contract that remains is
    the HTTP empty-scope envelope (returned by `HttpOracleEngine.ask` when the
    forwarder passes an empty scope). This test pins its key set so a future
    change cannot silently drop one of the operator-facing fields.
    """

    EXPECTED_KEYS = {
        "mode",
        "query",
        "summary",
        "answer",
        "citations",
        "not_found",
        "suggested_path",
        "results",
        "answer_source",
        "fallback_reason",
        "llm_provider",
        "llm_model",
    }

    def test_http_empty_scope_ask_envelope_is_complete(self):
        engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
        http_empty = engine.ask("q", 5, allowed_file_ids=set())
        self.assertEqual(
            set(http_empty.keys()),
            self.EXPECTED_KEYS,
            f"HTTP empty-scope envelope drifted: missing {self.EXPECTED_KEYS - set(http_empty.keys())}, extra {set(http_empty.keys()) - self.EXPECTED_KEYS}",
        )
        # The grounded-empty contract: no docs, no fabricated answer.
        self.assertEqual(http_empty["results"], [])
        self.assertIn("No Oracle documents are in scope", http_empty["summary"])


class HttpOracleEngineUnitTests(unittest.TestCase):
    def test_context_empty_scope_short_circuits_without_http(self):
        recorder = _Recorder({"query": "q", "chunks": [{"chunk_id": "x"}]})
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
            engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
            chunks = engine.context("q", 5, allowed_file_ids=set())
        self.assertEqual(chunks, [])
        self.assertEqual(recorder.calls, [])


class FilterForwardingTests(unittest.TestCase):
    """M3-P12c: bounded filters (kind/language/symbols/imports/module/group_by_file)
    are forwarded in the POST body only when non-None/non-False.

    expand_ckg is NOT forwarded (removed from the TOOLS schema, deferred to
    PLAN.md M1 max-recall "CKG expansion" ticket) — it is silently ignored
    without crashing and without appearing in the body.
    """

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

    def _make_engine(self, recorder):
        from oracle.server.aspis_mcp import HttpOracleEngine

        return HttpOracleEngine("http://127.0.0.1:8765", "tok")

    def test_context_forwards_kind_symbols(self):
        recorder = _Recorder({"query": "q", "chunks": [{"chunk_id": "c1"}]})
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
            engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
            engine.context(
                "q",
                5,
                allowed_file_ids={"a.py"},
                kind="function",
                symbols=["foo", "bar"],
            )
        self.assertEqual(len(recorder.calls), 1)
        body = recorder.calls[0]["json"]
        self.assertEqual(body["kind"], "function")
        self.assertEqual(body["symbols"], ["foo", "bar"])
        # These must be absent when not given.
        self.assertNotIn("language", body)
        self.assertNotIn("imports", body)
        self.assertNotIn("module", body)
        # group_by_file is ask-only (engine.context has no such arg).
        self.assertNotIn("group_by_file", body)

    def test_context_absent_filters_keeps_body_byte_identical(self):
        # Unfiltered call: body must be byte-identical to the pre-filter
        # shape (query/limit/allowed_file_ids only).
        recorder = _Recorder({"query": "q", "chunks": []})
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
            engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
            engine.context("q", 5, allowed_file_ids={"a.py"})
        body = recorder.calls[0]["json"]
        self.assertEqual(body, {"query": "q", "limit": 5, "allowed_file_ids": ["a.py"]})

    def test_ask_forwards_group_by_file(self):
        recorder = _Recorder(
            {
                "answer": "x",
                "citations": [],
                "not_found": False,
                "results": [],
                "mode": "oracle-http-bounded",
                "summary": "x",
                "suggested_path": None,
                "answer_source": None,
                "fallback_reason": None,
                "llm_provider": None,
                "llm_model": None,
            }
        )
        with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
            engine = HttpOracleEngine("http://127.0.0.1:8765", "tok")
            engine.ask("q", 5, allowed_file_ids={"a.py"}, group_by_file=True)
        body = recorder.calls[0]["json"]
        self.assertIs(body.get("group_by_file"), True)
        self.assertNotIn("kind", body)
        self.assertNotIn("symbols", body)

    def test_expand_ckg_in_args_is_ignored(self):
        # expand_ckg is NOT ported to the Rust engine; it must not appear in
        # the body and must not crash the dispatch. We verify via _parse_filter_args
        # (the actual choke point) rather than a direct engine call.
        from oracle.server.aspis_mcp import _parse_filter_args

        filters = _parse_filter_args({
            "kind": "function",
            "expand_ckg": True,  # must be silently dropped
        })
        self.assertEqual(filters, {"kind": "function"})
        self.assertNotIn("expand_ckg", filters)

    def test_parse_filter_args_ignores_expand_ckg(self):
        from oracle.server.aspis_mcp import _parse_filter_args

        filters = _parse_filter_args(
            {
                "kind": "function",
                "language": "rust",
                "symbols": ["foo"],
                "imports": ["bar"],
                "module": "backend",
                "group_by_file": True,
                "expand_ckg": True,  # must be silently dropped
            }
        )
        self.assertEqual(filters, {
            "kind": "function",
            "language": "rust",
            "symbols": ["foo"],
            "imports": ["bar"],
            "module": "backend",
            "group_by_file": True,
        })

    def test_parse_filter_args_empty_strings_stay_none(self):
        from oracle.server.aspis_mcp import _parse_filter_args

        filters = _parse_filter_args({
            "kind": "",
            "language": "   ",
            "symbols": [],
            "imports": ["", "  ", "valid"],
            "module": "",
            "group_by_file": False,
        })
        # kind/language/module empty -> absent; symbols empty list -> absent;
        # imports: only non-empty entries kept -> ["valid"].
        self.assertNotIn("kind", filters)
        self.assertNotIn("language", filters)
        self.assertNotIn("symbols", filters)
        self.assertEqual(filters.get("imports"), ["valid"])
        self.assertNotIn("module", filters)
        self.assertNotIn("group_by_file", filters)

    def test_dispatch_context_forwards_filters(self):
        # Full dispatch path: tool args -> _parse_filter_args -> engine body.
        recorder = _Recorder({"query": "q", "chunks": [{"chunk_id": "c1"}]})
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            with patch(
                "oracle.server.aspis_mcp.import_httpx",
                return_value=SimpleNamespace(Client=recorder.client),
            ):
                with patch(
                    "oracle.server.aspis_mcp.oracle_allowed_file_ids",
                    return_value={"a.py"},
                ):
                    handle_tool_call(
                        "oracle_context",
                        {
                            "query": "q",
                            "limit": 5,
                            "agent_id": "thin-agent",
                            "role": "orchestrator",
                            "kind": "struct",
                            "symbols": ["Widget"],
                            "imports": ["std::vec"],
                        },
                        root=root,
                    )
        self.assertEqual(len(recorder.calls), 1)
        body = recorder.calls[0]["json"]
        self.assertEqual(body["kind"], "struct")
        self.assertEqual(body["symbols"], ["Widget"])
        self.assertEqual(body["imports"], ["std::vec"])
        # group_by_file is ask-only; expand_ckg is dropped.
        self.assertNotIn("group_by_file", body)
        self.assertNotIn("expand_ckg", body)

    def test_dispatch_ask_forwards_filters(self):
        recorder = _Recorder(
            {
                "answer": "x",
                "citations": [],
                "not_found": False,
                "results": [],
                "mode": "oracle-http-bounded",
                "summary": "x",
                "suggested_path": None,
                "answer_source": None,
                "fallback_reason": None,
                "llm_provider": None,
                "llm_model": None,
            }
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            self._register(root)
            os.environ["ASPIS_ORACLE_HTTP_BASE"] = "http://127.0.0.1:8765"
            os.environ["ASPIS_ORACLE_AUTH_TOKEN"] = "env-token"
            with patch(
                "oracle.server.aspis_mcp.import_httpx",
                return_value=SimpleNamespace(Client=recorder.client),
            ):
                with patch(
                    "oracle.server.aspis_mcp.oracle_allowed_file_ids",
                    return_value={"a.py"},
                ):
                    handle_tool_call(
                        "oracle_ask",
                        {
                            "query": "q",
                            "limit": 3,
                            "agent_id": "thin-agent",
                            "role": "orchestrator",
                            "kind": "class",
                            "language": "rust",
                            "symbols": ["Foo"],
                            "imports": ["std::vec"],
                            "module": "backend",
                            "group_by_file": True,
                        },
                        root=root,
                    )
        self.assertEqual(len(recorder.calls), 1)
        body = recorder.calls[0]["json"]
        self.assertEqual(body["kind"], "class")
        self.assertEqual(body["language"], "rust")
        self.assertEqual(body["symbols"], ["Foo"])
        self.assertEqual(body["imports"], ["std::vec"])
        self.assertEqual(body["module"], "backend")
        self.assertIs(body.get("group_by_file"), True)
        self.assertNotIn("expand_ckg", body)


if __name__ == "__main__":
    unittest.main()
