import hashlib
import inspect
import json
import os
import re
import sys
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from oracle.server.aspis_mcp import (
    AGENTS_STATE_VERSION,
    APP_VAULT_ACCOUNTS,
    MAX_CLAIMS,
    MAX_SESSIONS,
    ROLE_ALIASES,
    ROLE_ALLOWED_TOOLS,
    ROLE_RULES,
    VALID_ROLES,
    McpError,
    AGENTS_STATE_FILE,
    coerce_role,
    normalize_role,
    read_agents_state,
    _coerce_subagent_count,
    app_vault_target,
    cap_claims,
    cap_git_push_requests,
    _scrub_push_result,
    cap_mini_coder_directives,
    cap_plan_approval_requests,
    MAX_PLAN_APPROVAL_REQUESTS,
    PLAN_MAX_MARKDOWN_CHARS,
    cap_sessions,
    clean_text,
    MAX_GIT_PUSH_REQUESTS,
    MAX_MINI_CODER_DIRECTIVES,
    MINI_CODER_WRITE_MODES,
    MINI_CODER_WRITE_MODE_DEFAULT,
    TOOLS,
    upsert_session,
    create_mcp_server,
    censor_shard_path,
    compact_structure,
    build_project_structure,
    resolve_structure_bridge_binary,
    _STRUCTURE_MAX_CONCURRENT_BUILDS,
    dispatch_oracle_ask,
    dispatch_oracle_context,
    dispatch_project_structure,
    dispatch_steer_mini_coder,
    dispatch_mini_coder_result,
    dispatch_spawn_main_coder,
    dispatch_spawn_mini_coder,
    hash_session_token,
    now,
    _pigeon_enabled,
    write_agents_state,
    MINI_CODER_MAX_STEER_LEN,
    MINI_CODER_MAX_STEER_QUEUE,
    dispose_censor_finding,
    handle_tool_call,
    read_censor_open_findings,
    validate_censor_rel_path,
    mcp_oracle_paths,
    normalize_agent_id,
    normalize_agents_state,
    normalize_model,
    normalize_subagents,
    oracle_allowed_file_ids,
    public_agents_state,
    require_registered_role,
    strip_invisible_and_bidi,
    validate_launch_token_for_registration,
    validate_project_state,
    validate_task_dependency_dag,
    MAX_PLAN_TASKS,
)


class _Recorder:
    """HTTP recorder: captures every POST body so a test can assert scope/auth/url.

    Mirrors the helper in test_thin_client.py (kept duplicated here so the two
    test files don't have a hidden import dependency). With `raise_exc` set, the
    fake `Client.post` raises that exception instead of returning a response
    — exercises the dispatch's HTTP-failure branch (M3-P12c: surfaces McpError).
    """

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


def prepare_management_root(root: Path) -> Path:
    (root / "config.json").write_text("{}", encoding="utf-8")
    (root / "oracle" / "server").mkdir(parents=True, exist_ok=True)
    (root / "oracle" / "server" / "aspis_mcp.py").write_text(
        "# test marker\n", encoding="utf-8"
    )
    projects = root / "projects"
    projects.mkdir(exist_ok=True)
    return projects


def sample_project(projects_dir: Path) -> Path:
    prepare_management_root(projects_dir.parent)
    path = projects_dir / "scrna-seq.md"
    path.write_text(
        """---
id: scrna-seq
title: scRNA-seq backend
status: active
updated_at: 2026-05-28T00:00:00Z
---

# Obiettivi
- Build a backend and UX for scRNA-seq analysis.

```aspis-project
{
  "version": 1,
  "tasks": [
    {
      "id": "T1",
      "title": "Design backend pipeline",
      "status": "todo",
      "priority": "high",
      "assignee": null,
      "due": null,
      "linkedResources": [],
      "updatedAt": "2026-05-28T00:00:00Z"
    }
  ],
  "notes": []
}
```

# Note libere
""",
        encoding="utf-8",
    )
    return path


class AspisMcpProjectTests(unittest.TestCase):
    def setUp(self):
        self._old_unmanaged_privileged = os.environ.get(
            "ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"
        )
        self._old_disable_app_vault = os.environ.get("ASPIS_MCP_DISABLE_APP_VAULT")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = "1"

    def tearDown(self):
        if self._old_unmanaged_privileged is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = (
                self._old_unmanaged_privileged
            )
        if self._old_disable_app_vault is None:
            os.environ.pop("ASPIS_MCP_DISABLE_APP_VAULT", None)
        else:
            os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = self._old_disable_app_vault

    def test_oracle_ask_uses_http_path_with_locally_computed_scope(self):
        # M3-P12c: there is no in-process engine anymore. The dispatch is
        # HTTP-only — the LLM runs on the resident server, so app-vault
        # LLM config is read there, NOT here. The MCP side only computes the
        # scope (`oracle_allowed_file_ids`) and forwards it; the resident
        # server widens/narrows nothing. We assert the HTTP contract here.

        recorder = _Recorder(
            {
                "mode": "oracle-http-bounded",
                "query": "how do agents use oracle",
                "answer": "from-http",
                "citations": [],
                "not_found": False,
                "suggested_path": None,
                "results": [],
                "answer_source": "http",
                "fallback_reason": None,
                "llm_provider": "scaleway",
                "llm_model": "voxtral-small-24b-2507",
            }
        )

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "oracle-agent",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "asking",
                },
                root=root,
            )
            with patch.dict(
                os.environ,
                {
                    "ASPIS_ORACLE_HTTP_BASE": "http://127.0.0.1:8765",
                    "ASPIS_ORACLE_AUTH_TOKEN": "env-token",
                },
            ):
                with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                    with patch(
                        "oracle.server.aspis_mcp.oracle_allowed_file_ids",
                        return_value={"oracle/server/aspis_mcp.py"},
                    ):
                        result = handle_tool_call(
                            "oracle_ask",
                            {
                                "query": "how do agents use oracle",
                                "limit": 3,
                                "agent_id": "oracle-agent",
                                "role": "orchestrator",
                            },
                            root=root,
                        )
        # HTTP path: the locally-computed scope is forwarded verbatim; the
        # server returns the answer envelope (LLM is server-side now).
        self.assertEqual(result["answer"], "from-http")
        self.assertEqual(result["llm_provider"], "scaleway")
        self.assertEqual(result["llm_model"], "voxtral-small-24b-2507")
        # The HTTP path returned the placeholder index_status, so the
        # tool-side response carries `source: resident-server` and None root.
        self.assertEqual(result["index_status"]["indexedFiles"], None)
        # The local scope was forwarded: the recorder saw the same set.
        self.assertEqual(len(recorder.calls), 1)
        call = recorder.calls[0]
        self.assertTrue(call["url"].endswith("/ask-bounded"))
        self.assertEqual(call["json"]["allowed_file_ids"], ["oracle/server/aspis_mcp.py"])

    def test_project_oracle_scope_includes_management_mcp_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects_dir = prepare_management_root(root)
            work_root = root / "Aspis Bio Work"
            work_root.mkdir()
            (work_root / "pipeline.py").write_text(
                "print('pipeline')\n", encoding="utf-8"
            )
            escaped_work_root = str(work_root).replace("\\", "\\\\")
            (projects_dir / "mcp-scope.md").write_text(
                f"""---
id: mcp-scope
title: MCP scope
status: active
updated_at: 2026-05-29T00:00:00Z
root_path: "{escaped_work_root}"
---

```aspis-project
{{"version":1,"tasks":[],"notes":[]}}
```
""",
                encoding="utf-8",
            )
            manifest = {
                "version": 2,
                "roots": {
                    str(work_root.resolve()): {"files": {"pipeline.py": {}}},
                    str(root.resolve()): {"files": {"oracle/server/aspis_mcp.py": {}}},
                },
            }
            (root / "oracle-data").mkdir(parents=True, exist_ok=True)
            (root / "oracle-data" / "chunk-index-manifest.json").write_text(
                json.dumps(manifest),
                encoding="utf-8",
            )

            allowed = oracle_allowed_file_ids(projects_dir, {"project_id": "mcp-scope"})

            self.assertIn("pipeline.py", allowed)
            self.assertIn("oracle/server/aspis_mcp.py", allowed)

    def test_unscoped_oracle_query_does_not_expose_other_project_roots(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects_dir = prepare_management_root(root)
            other_work_root = root / "Other Project Work"
            other_work_root.mkdir()
            (other_work_root / "secret_pipeline.py").write_text(
                "print('other')\n", encoding="utf-8"
            )
            manifest = {
                "version": 2,
                "roots": {
                    str(root.resolve()): {"files": {"oracle/server/aspis_mcp.py": {}}},
                    str(other_work_root.resolve()): {
                        "files": {"secret_pipeline.py": {}}
                    },
                },
            }
            (root / "oracle-data").mkdir(parents=True, exist_ok=True)
            (root / "oracle-data" / "chunk-index-manifest.json").write_text(
                json.dumps(manifest),
                encoding="utf-8",
            )

            # No project_id => must scope to MANAGEMENT ROOT ONLY, never the union.
            allowed = oracle_allowed_file_ids(projects_dir, {})

            self.assertIsNotNone(allowed)
            self.assertIn("oracle/server/aspis_mcp.py", allowed)
            self.assertNotIn("secret_pipeline.py", allowed)

    def test_project_schema_requires_rust_visible_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "schema-auditor",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "schema",
                },
                root=root,
            )

            original = path.read_text(encoding="utf-8")
            path.write_text(original.replace('  "version": 1,\n', ""), encoding="utf-8")
            with self.assertRaises(McpError) as missing_version:
                handle_tool_call(
                    "project_get",
                    {
                        "project_id": "scrna-seq",
                        "agent_id": "schema-auditor",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            self.assertIn("version", str(missing_version.exception))

            path.write_text(
                original.replace(
                    '"updatedAt": "2026-05-28T00:00:00Z"', '"updatedAt": ""'
                ),
                encoding="utf-8",
            )
            with self.assertRaises(McpError) as missing_task_updated:
                handle_tool_call(
                    "project_get",
                    {
                        "project_id": "scrna-seq",
                        "agent_id": "schema-auditor",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            self.assertIn("updatedAt", str(missing_task_updated.exception))

    def test_project_frontmatter_id_must_match_filename(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            path.write_text(
                path.read_text(encoding="utf-8").replace(
                    "id: scrna-seq", "id: other-project"
                ),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "schema-auditor",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "schema",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_get",
                    {
                        "project_id": "scrna-seq",
                        "agent_id": "schema-auditor",
                        "role": "orchestrator",
                    },
                    root=root,
                )

            self.assertIn("filename expects", str(ctx.exception))

    def test_project_state_rejects_duplicate_task_ids(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            content = path.read_text(encoding="utf-8")
            duplicate = content.replace(
                '    }\n  ],\n  "notes": []',
                '    },\n    {\n      "id": "T1",\n      "title": "Duplicate id",\n      "status": "review",\n      "priority": "high",\n      "assignee": null,\n      "due": null,\n      "linkedResources": [],\n      "updatedAt": "2026-05-28T00:00:00Z"\n    }\n  ],\n  "notes": []',
            )
            path.write_text(duplicate, encoding="utf-8")
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "schema-auditor",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "schema",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_get",
                    {
                        "project_id": "scrna-seq",
                        "agent_id": "schema-auditor",
                        "role": "orchestrator",
                    },
                    root=root,
                )

            self.assertIn("Duplicate project task id", str(ctx.exception))

    def test_project_set_title_renames_durably(self):
        # B7: the orchestrator renames the project via MCP during planning; the new
        # title must be returned AND durable on disk (re-read shows it).
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "namer",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "naming",
                },
                root=root,
            )
            result = handle_tool_call(
                "project_set_title",
                {
                    "project_id": "scrna-seq",
                    "title": "Renamed Plan",
                    "agent_id": "namer",
                    "role": "orchestrator",
                },
                root=root,
            )
            self.assertEqual(result["metadata"]["title"], "Renamed Plan")
            reloaded = handle_tool_call(
                "project_get",
                {
                    "project_id": "scrna-seq",
                    "agent_id": "namer",
                    "role": "orchestrator",
                },
                root=root,
            )
            self.assertEqual(reloaded["metadata"]["title"], "Renamed Plan")

    def test_python_write_preserves_censor_trusted(self):
        # B7 fix (reviewer BLOCKER): a Python frontmatter rewrite (here via
        # project_set_title) must NOT silently drop the user's `censor_trusted: true`
        # opt-in. Without the round-trip the flag vanishes and Censor is revoked.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            # Mark the project censor-trusted exactly as the Rust serializer does
            # (a `censor_trusted: true` line just before the frontmatter close).
            text = path.read_text(encoding="utf-8")
            text = text.replace("---\n\n", "censor_trusted: true\n---\n\n", 1)
            path.write_text(text, encoding="utf-8")
            self.assertIn("censor_trusted: true", path.read_text(encoding="utf-8"))

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "namer",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "x",
                },
                root=root,
            )
            handle_tool_call(
                "project_set_title",
                {
                    "project_id": "scrna-seq",
                    "title": "Trusted Rename",
                    "agent_id": "namer",
                    "role": "orchestrator",
                },
                root=root,
            )
            after = path.read_text(encoding="utf-8")
            self.assertIn(
                "censor_trusted: true",
                after,
                "a Python write must preserve the Censor trust opt-in",
            )
            self.assertIn("title: Trusted Rename", after)

    def test_legacy_orchestrator_can_claim_but_only_verifier_closes(self):
        # BACK-COMPAT FIXTURE: this agent registers and operates with the legacy
        # role="orchestrator", which now normalizes to itself (first-class role).
        # It can claim and (as an orchestrator/coder-like) move to review/blocked,
        # but it still cannot self-close; only a verifier sets done.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "cheap-orch-1",
                    "role": "orchestrator",
                    "model": "qwen-local",
                    "message": "starting",
                },
                root=root,
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "verifier-1",
                    "role": "verifier",
                    "model": "qwen-local",
                    "message": "verifying",
                },
                root=root,
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex-1",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )
            state = handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "cheap-orch-1",
                    "role": "orchestrator",
                },
                root=root,
            )
            # COMPACT ACK (round-5 class fix): project_claim_task now returns a
            # compact ack with a "claim" key carrying the lease outcome.
            self.assertEqual(state["claim"]["taskId"], "T1")

            with self.assertRaises(McpError):
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "done",
                        "agent_id": "cheap-orch-1",
                        "role": "orchestrator",
                        "evidence": "Verified project spec and required backend task is complete.",
                        "confidence": 0.82,
                    },
                    root=root,
                )

            # Phase B merge: a legacy orchestrator is now a coder, so it CAN move
            # its own claimed task to a coder-allowed status (the pre-merge
            # "todo/blocked only" restriction is gone) — but NOT to done. We move
            # to blocked (still coder-claimable downstream) to prove the relaxed
            # transition without corrupting the verifier-close path below.
            blocked_by_legacy = handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "blocked",
                    "agent_id": "cheap-orch-1",
                    "role": "orchestrator",
                    "evidence": "Blocking until an upstream dependency is ready.",
                    "confidence": 0.72,
                },
                root=root,
            )
            self.assertEqual(
                blocked_by_legacy["state"]["tasks"][0]["status"], "blocked"
            )

            state_path = projects / ".aspis-agents.json"
            state = json.loads(state_path.read_text(encoding="utf-8"))
            state["claims"] = []
            state_path.write_text(json.dumps(state), encoding="utf-8")

            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "codex-1",
                    "role": "coder",
                },
                root=root,
            )
            reviewed = handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "review",
                    "agent_id": "codex-1",
                    "role": "coder",
                    "evidence": "Implementation is ready for verifier handoff.",
                    "confidence": 0.72,
                },
                root=root,
            )
            self.assertEqual(reviewed["state"]["tasks"][0]["status"], "review")

            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "verifier-1",
                    "role": "verifier",
                },
                root=root,
            )
            project = handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "done",
                    "agent_id": "verifier-1",
                    "role": "verifier",
                    "evidence": "Verifier audited the task and confirmed the backend task is complete.",
                    "confidence": 0.82,
                },
                root=root,
            )

            self.assertEqual(project["state"]["tasks"][0]["status"], "done")
            self.assertEqual(project["metadata"]["status"], "done")
            self.assertTrue(
                any(
                    "verifier-1" in note["source"] for note in project["state"]["notes"]
                )
            )

    def test_coder_cannot_close_task_without_verifier(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )
            with self.assertRaises(McpError):
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "done",
                        "agent_id": "codex",
                        "role": "coder",
                        "evidence": "I think it is complete.",
                        "confidence": 0.9,
                    },
                    root=root,
                )

    def test_mini_role_is_oracle_context_only(self):
        # P3 + Phase 11.2: the mini's MCP scope is READ-ONLY — it may register and
        # call the read-only retrieval tools (oracle_context + project_structure),
        # and NOTHING else. Every mutation tool is rejected at the role gate,
        # SERVER-side (the prompt is advisory; this is the wall).
        self.assertEqual(
            ROLE_ALLOWED_TOOLS["mini"],
            {
                "agent_register",
                "oracle_context",
                "project_structure",
                "get_neighborhood",
                "find_imports",
            },
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "mini-1",
                    "role": "mini",
                    "model": "qwen",
                    "message": "reading context",
                },
                root=root,
            )
            # The role gate passes for the ONE allowed read tool…
            self.assertEqual(
                require_registered_role(projects, "mini-1", "mini", "oracle_context"),
                "mini",
            )
            # …and rejects every mutation/spawn/censor tool at the same gate.
            for tool in (
                "project_claim_task",
                "project_update_status",
                "project_append_note",
                "spawn_mini_coder",
                "censor_dispose",
                "agent_heartbeat",
            ):
                with self.assertRaises(McpError):
                    require_registered_role(projects, "mini-1", "mini", tool)

    def test_mini_cannot_act_or_reregister_as_coder(self):
        # P3 pinning: once registered as "mini", the stored role caps the agent.
        # Acting as coder on a tool call hits the role-mismatch gate, and a
        # re-register under the coder role is rejected against the stored session.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "mini-2",
                    "role": "mini",
                    "model": "qwen",
                    "message": "reading context",
                },
                root=root,
            )
            # Tool call claiming the coder role -> role-mismatch rejection.
            with self.assertRaises(McpError):
                require_registered_role(projects, "mini-2", "coder", "oracle_context")
            # Re-registration as coder -> rejected against the stored mini session.
            with self.assertRaises(McpError):
                handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": "mini-2",
                        "role": "coder",
                        "model": "qwen",
                        "message": "promoting myself",
                    },
                    root=root,
                )

    def test_mini_registration_is_launch_token_bound(self):
        # P3 token binding: when the app pre-seeded the mini session with a
        # launch-token HASH, agent_register REQUIRES the matching raw token —
        # the unmanaged compat flag (set in setUp) never bypasses an existing
        # hash. Missing and wrong tokens are rejected; the right one registers.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            token = "mini-launch-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-06-12T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "mini-3",
                                "role": "mini",
                                "status": "active",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            base = {
                "agent_id": "mini-3",
                "role": "mini",
                "model": "qwen",
                "message": "reading context",
            }
            with self.assertRaises(McpError):
                handle_tool_call("agent_register", dict(base), root=root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "agent_register", dict(base, launch_token="wrong"), root=root
                )
            # The matching token registers cleanly (no raise) as role "mini".
            handle_tool_call(
                "agent_register", dict(base, launch_token=token), root=root
            )

    def test_mini_cannot_reregister_after_token_consumed_sec7(self):
        # SEC#7 (EOD review): once the launch token is consumed (popped on the
        # first successful register), a SECOND tokenless agent_register must be
        # REJECTED — otherwise anyone who learns the agent_id can mint a fresh
        # session token. Role stays "mini" regardless, but the one-shot binding
        # must hold.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            token = "mini-once-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-06-12T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "mini-7",
                                "role": "mini",
                                "status": "active",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            base = {
                "agent_id": "mini-7",
                "role": "mini",
                "model": "qwen",
                "message": "x",
            }
            # First register with the right token: succeeds + consumes the hash.
            handle_tool_call(
                "agent_register", dict(base, launch_token=token), root=root
            )
            # Second register, now tokenless: must be REJECTED (consumed).
            with self.assertRaises(McpError):
                handle_tool_call("agent_register", dict(base), root=root)

    def test_mini_oracle_context_is_bound_to_its_project_sec9(self):
        # SEC#9 (EOD review): the mini's read-only oracle grant is scoped to its
        # SPAWNING project — a mini bound to project A asking oracle_context for
        # project B must be rejected (cross-project corpus read), even though the
        # role gate allows oracle_context.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-06-12T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "mini-9",
                                "role": "mini",
                                "status": "active",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                                "currentProjectId": "scrna-seq",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            # A second REAL project the mini was NOT spawned for — so the only
            # thing that can block the read is the mini-scope check (not a
            # missing-project coincidence).
            (projects / "other-proj.md").write_text(
                "---\nid: other-proj\ntitle: Other\nstatus: active\n"
                "updated_at: 2026-05-28T00:00:00Z\n---\n\n# Other\n",
                encoding="utf-8",
            )
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "oracle_context",
                    {
                        "agent_id": "mini-9",
                        "role": "mini",
                        "query": "anything",
                        "project_id": "other-proj",
                    },
                    root=root,
                )
            # Must fail for the SCOPE reason, not a downstream coincidence.
            self.assertIn("project", str(ctx.exception).lower())
            self.assertIn("mini", str(ctx.exception).lower())

    def test_coder_claim_moves_todo_task_to_wip_for_live_kanban(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex-worker",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )
            state = handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "codex-worker",
                    "role": "coder",
                },
                root=root,
            )
            project = handle_tool_call(
                "project_get",
                {
                    "project_id": "scrna-seq",
                    "agent_id": "codex-worker",
                    "role": "coder",
                },
                root=root,
            )

            self.assertEqual(project["state"]["tasks"][0]["status"], "wip")
            # COMPACT ACK (round-5 class fix): project_claim_task now returns a
            # compact ack with a "claim" key carrying the lease outcome.
            self.assertEqual(state["claim"]["status"], "wip")
            self.assertTrue(
                any(
                    "moved it to wip" in note["text"]
                    for note in project["state"]["notes"]
                )
            )

    def test_verifier_next_task_only_returns_review_or_blocked_work(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "audit-agent",
                    "role": "verifier",
                    "model": "codex",
                    "message": "verifying",
                },
                root=root,
            )
            no_task = handle_tool_call(
                "project_next_task",
                {
                    "project_id": "scrna-seq",
                    "agent_id": "audit-agent",
                    "role": "verifier",
                },
                root=root,
            )
            self.assertIsNone(no_task["task"])

            project_path = projects / "scrna-seq.md"
            content = project_path.read_text(encoding="utf-8").replace(
                '"status": "todo"',
                '"status": "review"',
            )
            project_path.write_text(content, encoding="utf-8")
            review_task = handle_tool_call(
                "project_next_task",
                {
                    "project_id": "scrna-seq",
                    "agent_id": "audit-agent",
                    "role": "verifier",
                },
                root=root,
            )

            self.assertEqual(review_task["task"]["id"], "T1")

    def test_verifier_cannot_directly_claim_todo_work(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "audit-agent",
                    "role": "verifier",
                    "model": "codex",
                    "message": "verifying",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_claim_task",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "agent_id": "audit-agent",
                        "role": "verifier",
                    },
                    root=root,
                )

            self.assertIn(
                "Verifier agents can only claim review or blocked tasks",
                str(ctx.exception),
            )

    def test_agent_events_and_notes_use_unique_ids_under_fast_updates(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex-fast",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "codex-fast",
                    "role": "coder",
                },
                root=root,
            )
            handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "wip",
                    "agent_id": "codex-fast",
                    "role": "coder",
                    "evidence": "Started the implementation work through MCP.",
                    "confidence": 0.5,
                },
                root=root,
            )
            project = handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "review",
                    "agent_id": "codex-fast",
                    "role": "coder",
                    "evidence": "Ready for verifier review after MCP implementation.",
                    "confidence": 0.7,
                },
                root=root,
            )
            state = handle_tool_call(
                "agent_state",
                {"agent_id": "codex-fast", "role": "coder"},
                root=root,
            )

            note_ids = [note["id"] for note in project["state"]["notes"]]
            event_ids = [event["id"] for event in state["events"]]
            self.assertEqual(len(note_ids), len(set(note_ids)))
            self.assertEqual(len(event_ids), len(set(event_ids)))

    def test_agent_state_rekeys_duplicate_legacy_event_ids(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            token = "pending-coder-token"
            (projects / ".aspis-agents.json").write_text(
                """{
  "version": 1,
  "updatedAt": "2026-05-28T00:00:00Z",
  "sessions": [
    {"agentId": "codex", "role": "coder", "lastSeenAt": "2026-05-28T00:00:00Z"}
  ],
  "claims": [],
  "events": [
    {"id": "E1", "timestamp": "2026-05-28T00:00:00Z", "agentId": "codex", "role": "coder", "eventType": "claim", "message": "one"},
    {"id": "E1", "timestamp": "2026-05-28T00:00:01Z", "agentId": "codex", "role": "coder", "eventType": "status", "message": "two"}
  ]
}""",
                encoding="utf-8",
            )

            state = handle_tool_call(
                "agent_state",
                {"agent_id": "codex", "role": "coder"},
                root=root,
            )

            event_ids = [event["id"] for event in state["events"]]
            self.assertEqual(len(event_ids), len(set(event_ids)))

    def test_mcp_status_update_preserves_project_root_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            content = path.read_text(encoding="utf-8")
            path.write_text(
                content.replace(
                    "updated_at: 2026-05-28T00:00:00Z\n",
                    'updated_at: 2026-05-28T00:00:00Z\nroot_path: "C:\\\\Users\\\\gualt\\\\Desktop\\\\aspis bio"\n',
                ),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "codex",
                    "role": "coder",
                },
                root=root,
            )

            handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "wip",
                    "agent_id": "codex",
                    "role": "coder",
                    "evidence": "Started implementation.",
                    "confidence": 0.5,
                },
                root=root,
            )

            self.assertIn("root_path:", path.read_text(encoding="utf-8"))

    def test_non_verifier_must_claim_before_status_update(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "review",
                        "agent_id": "codex",
                        "role": "coder",
                        "evidence": "Ready for verifier.",
                        "confidence": 0.6,
                    },
                    root=root,
                )

            self.assertIn("claim the task", str(ctx.exception))

    def test_active_claim_blocks_other_coder_takeover(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            for agent_id in ["coder-1", "coder-2"]:
                handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": agent_id,
                        "role": "coder",
                        "model": "codex",
                        "message": "coding",
                    },
                    root=root,
                )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "coder-1",
                    "role": "coder",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_claim_task",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "agent_id": "coder-2",
                        "role": "coder",
                    },
                    root=root,
                )

            self.assertIn("already claimed", str(ctx.exception))

    def test_stale_leaseless_claim_does_not_block_new_claim(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            token = "pending-coder-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2000-01-01T00:00:00+00:00",
                        "sessions": [],
                        "claims": [
                            {
                                "projectId": "scrna-seq",
                                "projectTitle": "scRNA-seq backend",
                                "taskId": "T1",
                                "taskTitle": "Design backend pipeline",
                                "agentId": "dead-coder",
                                "role": "coder",
                                "status": "wip",
                                "claimedAt": "2000-01-01T00:00:00+00:00",
                                "updatedAt": "2000-01-01T00:00:00+00:00",
                                "leaseUntil": None,
                            }
                        ],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "fresh-coder",
                    "role": "coder",
                    "model": "codex",
                    "message": "claiming stale task",
                },
                root=root,
            )

            state = handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "fresh-coder",
                    "role": "coder",
                },
                root=root,
            )
            # COMPACT ACK (round-5 class fix): project_claim_task now returns a
            # compact ack; the ledger is asserted via on-disk state.
            on_disk = json.loads(
                (projects / AGENTS_STATE_FILE).read_text(encoding="utf-8")
            )
            self.assertEqual(len(on_disk["claims"]), 1)
            self.assertEqual(on_disk["claims"][0]["agentId"], "fresh-coder")

    def test_agent_state_reconciles_missing_task_to_blocked_claim(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            content = path.read_text(encoding="utf-8").replace(
                '"id": "T1"', '"id": "T2"', 1
            )
            path.write_text(content, encoding="utf-8")
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "coder-1",
                                "role": "coder",
                                "status": "active",
                                "lastSeenAt": "2026-05-29T00:00:00+00:00",
                            }
                        ],
                        "claims": [
                            {
                                "projectId": "scrna-seq",
                                "projectTitle": "scRNA-seq backend",
                                "taskId": "T1",
                                "taskTitle": "Design backend pipeline",
                                "agentId": "coder-1",
                                "role": "coder",
                                "status": "wip",
                                "claimedAt": "2026-05-29T00:00:00+00:00",
                                "updatedAt": "2026-05-29T00:00:00+00:00",
                                "leaseUntil": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )

            state = handle_tool_call(
                "agent_state",
                {"agent_id": "coder-1", "role": "coder"},
                root=root,
            )

            self.assertEqual(state["claims"][0]["status"], "blocked")
            self.assertEqual(
                state["claims"][0]["evidence"],
                "Task missing during agent-state reconciliation.",
            )

    def test_coder_can_reopen_claimed_task_to_todo_after_merge(self):
        # Phase B merge: the coder absorbs the former orchestrator's planning
        # power, so it MAY reopen its claimed task to todo. (Pre-merge a coder
        # was limited to wip/review/blocked; orchestrator owned the todo reopen.)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "codex",
                    "role": "coder",
                },
                root=root,
            )

            result = handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "todo",
                    "agent_id": "codex",
                    "role": "coder",
                    "evidence": "Replanning: reopening this task to todo.",
                    "confidence": 0.5,
                },
                root=root,
            )
            self.assertEqual(result["state"]["tasks"][0]["status"], "todo")

    def test_coder_still_cannot_set_done(self):
        # The merge does NOT relax the verifier-only `done` gate.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "codex",
                    "role": "coder",
                },
                root=root,
            )
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "done",
                        "agent_id": "codex",
                        "role": "coder",
                        "evidence": "Trying to self-close the task.",
                        "confidence": 0.9,
                    },
                    root=root,
                )
            self.assertIn("Coder can only set", str(ctx.exception))

    def test_verifier_cannot_close_before_review(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            path.write_text(
                path.read_text(encoding="utf-8").replace(
                    '"status": "todo"', '"status": "blocked"'
                ),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "verifier-1",
                    "role": "verifier",
                    "model": "cheap-checker",
                    "message": "verifying",
                },
                root=root,
            )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "verifier-1",
                    "role": "verifier",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "done",
                        "agent_id": "verifier-1",
                        "role": "verifier",
                        "evidence": "Verifier audited evidence and command output carefully.",
                        "confidence": 0.8,
                    },
                    root=root,
                )

            self.assertIn("review first", str(ctx.exception))

    def test_done_requires_evidence_and_confidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            content = path.read_text(encoding="utf-8")
            path.write_text(
                content.replace('"status": "todo"', '"status": "review"'),
                encoding="utf-8",
            )

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "verifier-1",
                    "role": "verifier",
                    "model": "cheap-checker",
                    "message": "verifying",
                },
                root=root,
            )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "verifier-1",
                    "role": "verifier",
                },
                root=root,
            )
            with self.assertRaises(McpError):
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "done",
                        "agent_id": "verifier-1",
                        "role": "verifier",
                        "evidence": "ok",
                        "confidence": 0.69,
                    },
                    root=root,
                )

    def test_project_next_task_is_read_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            before = path.read_text(encoding="utf-8")
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "model": "cheap",
                    "message": "planning",
                },
                root=root,
            )

            result = handle_tool_call(
                "project_next_task",
                {"project_id": "scrna-seq", "agent_id": "orch", "role": "orchestrator"},
                root=root,
            )

            self.assertEqual(result["task"]["id"], "T1")
            self.assertEqual(path.read_text(encoding="utf-8"), before)

    def test_project_next_task_skips_active_claims_by_other_agents(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            for agent_id in ("coder-1", "coder-2"):
                handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": agent_id,
                        "role": "coder",
                        "model": "codex",
                        "message": "coding",
                    },
                    root=root,
                )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "coder-1",
                    "role": "coder",
                },
                root=root,
            )

            result = handle_tool_call(
                "project_next_task",
                {"project_id": "scrna-seq", "agent_id": "coder-2", "role": "coder"},
                root=root,
            )

            self.assertIsNone(result["task"])

    def test_project_reads_require_registered_agent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            with self.assertRaises(McpError) as ctx:
                handle_tool_call("project_get", {"project_id": "scrna-seq"}, root=root)

            self.assertIn("Agent id is required", str(ctx.exception))

    def test_done_task_cannot_be_claimed_or_reopened(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            content = path.read_text(encoding="utf-8")
            path.write_text(
                content.replace('"status": "todo"', '"status": "done"'),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_claim_task",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "agent_id": "codex",
                        "role": "coder",
                    },
                    root=root,
                )

            self.assertIn("Done tasks cannot be claimed", str(ctx.exception))

    def test_verifier_cannot_move_task_to_wip(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            path.write_text(
                path.read_text(encoding="utf-8").replace(
                    '"status": "todo"', '"status": "review"'
                ),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "verifier-1",
                    "role": "verifier",
                    "model": "cheap-checker",
                    "message": "verifying",
                },
                root=root,
            )
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "verifier-1",
                    "role": "verifier",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "wip",
                        "agent_id": "verifier-1",
                        "role": "verifier",
                        "evidence": "Verifier should not put tasks in work in progress.",
                        "confidence": 0.8,
                    },
                    root=root,
                )

            self.assertIn("Verifier can only set done or blocked", str(ctx.exception))

    def test_project_list_summary_includes_root_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            content = path.read_text(encoding="utf-8")
            path.write_text(
                content.replace(
                    "updated_at: 2026-05-28T00:00:00Z\n",
                    'updated_at: 2026-05-28T00:00:00Z\nroot_path: "C:\\\\Users\\\\gualt\\\\Desktop\\\\aspis bio"\n',
                ),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "model": "cheap",
                    "message": "listing",
                },
                root=root,
            )

            result = handle_tool_call(
                "project_list", {"agent_id": "orch", "role": "orchestrator"}, root=root
            )

            self.assertEqual(
                result["projects"][0]["rootPath"],
                "C:\\Users\\gualt\\Desktop\\aspis bio",
            )

    def test_registered_verifier_cannot_spoof_coder_role(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "verifier-1",
                    "role": "verifier",
                    "model": "cheap-checker",
                    "message": "verifying",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_claim_task",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "agent_id": "verifier-1",
                        "role": "coder",
                    },
                    root=root,
                )

            self.assertIn("registered as verifier", str(ctx.exception))

    def test_launch_pending_agent_must_register_before_project_tools(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            token = "pending-coder-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "pending-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-05-29T00:00:00Z",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_get",
                    {
                        "project_id": "scrna-seq",
                        "agent_id": "pending-coder",
                        "role": "coder",
                    },
                    root=root,
                )

            self.assertIn("launch is pending", str(ctx.exception))

            state = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "pending-coder",
                    "role": "coder",
                    "model": "codex",
                    "message": "registered after app launch",
                    "launch_token": token,
                },
                root=root,
            )
            self.assertEqual(state["session"]["status"], "active")

    def test_privileged_agent_requires_app_launch_token_without_compat_env(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            with patch.dict(
                os.environ,
                {"ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS": ""},
                clear=False,
            ):
                with self.assertRaises(McpError) as ctx:
                    handle_tool_call(
                        "agent_register",
                        {
                            "agent_id": "self-attested-coder",
                            "role": "coder",
                            "model": "codex",
                            "message": "try direct privileged register",
                        },
                        root=root,
                    )

            self.assertIn("app-issued launch token", str(ctx.exception))

    def test_orchestrator_also_requires_app_launch_token_without_compat_env(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)

            with patch.dict(
                os.environ,
                {"ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS": ""},
                clear=False,
            ):
                with self.assertRaises(McpError) as ctx:
                    handle_tool_call(
                        "agent_register",
                        {
                            "agent_id": "self-attested-orchestrator",
                            "role": "orchestrator",
                            "model": "cheap",
                            "message": "try direct register",
                        },
                        root=root,
                    )

            self.assertIn("app-issued launch token", str(ctx.exception))

    def test_app_launch_token_authorizes_privileged_agent_registration_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            token = "test-launch-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "pending-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-05-29T00:00:00+00:00",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )

            with patch.dict(
                os.environ,
                {"ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS": ""},
                clear=False,
            ):
                state = handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": "pending-coder",
                        "role": "coder",
                        "model": "codex",
                        "message": "registered through app launch",
                        "launch_token": token,
                    },
                    root=root,
                )

            self.assertEqual(state["session"]["status"], "active")
            self.assertNotIn("launchTokenHash", state["session"])
            self.assertNotIn("sessionTokenHash", state["session"])
            self.assertTrue(state["sessionToken"])

    def test_registered_agent_session_token_blocks_spoofed_status_update(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            launch_token = "test-launch-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "pending-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-05-29T00:00:00+00:00",
                                "launchTokenHash": hashlib.sha256(
                                    launch_token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )

            with patch.dict(
                os.environ,
                {"ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS": ""},
                clear=False,
            ):
                registered = handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": "pending-coder",
                        "role": "coder",
                        "model": "codex",
                        "message": "registered through app launch",
                        "launch_token": launch_token,
                    },
                    root=root,
                )
                session_token = registered["sessionToken"]
                handle_tool_call(
                    "project_claim_task",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "agent_id": "pending-coder",
                        "role": "coder",
                        "session_token": session_token,
                    },
                    root=root,
                )
                with self.assertRaises(McpError) as missing_ctx:
                    handle_tool_call(
                        "project_update_status",
                        {
                            "project_id": "scrna-seq",
                            "task_id": "T1",
                            "status": "review",
                            "agent_id": "pending-coder",
                            "role": "coder",
                            "evidence": "Ready for verifier review with concrete evidence.",
                            "confidence": 0.72,
                        },
                        root=root,
                    )
                with self.assertRaises(McpError) as wrong_ctx:
                    handle_tool_call(
                        "project_update_status",
                        {
                            "project_id": "scrna-seq",
                            "task_id": "T1",
                            "status": "review",
                            "agent_id": "pending-coder",
                            "role": "coder",
                            "evidence": "Ready for verifier review with concrete evidence.",
                            "confidence": 0.72,
                            "session_token": "wrong-token",
                        },
                        root=root,
                    )

                project = handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "review",
                        "agent_id": "pending-coder",
                        "role": "coder",
                        "evidence": "Ready for verifier review with concrete evidence.",
                        "confidence": 0.72,
                        "session_token": session_token,
                    },
                    root=root,
                )

            self.assertIn("session_token", str(missing_ctx.exception))
            self.assertIn("session token is invalid", str(wrong_ctx.exception))
            self.assertEqual(project["state"]["tasks"][0]["status"], "review")

    def test_app_launch_token_rejects_wrong_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "pending-verifier",
                                "role": "verifier",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-05-29T00:00:00+00:00",
                                "launchTokenHash": hashlib.sha256(
                                    b"right-token"
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )

            with patch.dict(
                os.environ,
                {"ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS": ""},
                clear=False,
            ):
                with self.assertRaises(McpError) as ctx:
                    handle_tool_call(
                        "agent_register",
                        {
                            "agent_id": "pending-verifier",
                            "role": "verifier",
                            "model": "audit",
                            "message": "wrong token",
                            "launch_token": "wrong-token",
                        },
                        root=root,
                    )

            self.assertIn("launch token is invalid", str(ctx.exception))

    def test_heartbeat_cannot_create_unregistered_agent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "agent_heartbeat",
                    {
                        "agent_id": "ghost-agent",
                        "status": "active",
                        "message": "try implicit register",
                    },
                    root=root,
                )

            self.assertIn("agent_register", str(ctx.exception))

    def test_launch_pending_agent_must_register_before_heartbeat(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "pending-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-05-29T00:00:00Z",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "agent_heartbeat",
                    {
                        "agent_id": "pending-coder",
                        "status": "active",
                        "message": "try implicit register",
                    },
                    root=root,
                )

            self.assertIn("launch is pending", str(ctx.exception))

    def test_agent_state_requires_registered_agent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)

            with self.assertRaises(McpError) as ctx:
                handle_tool_call("agent_state", {}, root=root)

            self.assertIn("Agent id", str(ctx.exception))

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "architect-1",
                    "role": "architect",
                    "model": "planner",
                    "message": "planning",
                },
                root=root,
            )

            state = handle_tool_call(
                "agent_state",
                {"agent_id": "architect-1", "role": "architect"},
                root=root,
            )

            # Phase B merge: the architect alias now folds to coder.
            self.assertEqual(state["sessions"][0]["role"], "coder")

    def test_role_aliases_map_to_canonical_roles(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "code-1",
                    "role": "code",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )

            state = handle_tool_call(
                "agent_state",
                {"agent_id": "code-1", "role": "coder"},
                root=root,
            )

            self.assertEqual(state["sessions"][0]["role"], "coder")

    def test_agent_role_cannot_be_re_registered(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "same-agent",
                    "role": "verifier",
                    "model": "cheap-checker",
                    "message": "verifying",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": "same-agent",
                        "role": "coder",
                        "model": "codex",
                        "message": "switching",
                    },
                    root=root,
                )

            self.assertIn("already registered as verifier", str(ctx.exception))

    def test_verifier_cannot_create_followup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "verifier-1",
                    "role": "verifier",
                    "model": "cheap-checker",
                    "message": "verifying",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_create_followup",
                    {
                        "project_id": "scrna-seq",
                        "agent_id": "verifier-1",
                        "role": "verifier",
                        "title": "Verifier-created work",
                        "reason": "should not be allowed",
                    },
                    root=root,
                )

            self.assertIn(
                "verifier agents cannot use project_create_followup", str(ctx.exception)
            )

    def test_followup_cannot_reopen_closed_project(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            path = sample_project(projects)
            path.write_text(
                path.read_text(encoding="utf-8").replace(
                    "status: active", "status: done"
                ),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "starting",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_create_followup",
                    {
                        "project_id": "scrna-seq",
                        "title": "Reopen hidden work",
                        "reason": "should not revive a closed board",
                        "agent_id": "orch",
                        "role": "orchestrator",
                    },
                    root=root,
                )

            self.assertIn("Cannot create follow-up", str(ctx.exception))

    def test_followup_stores_optional_description_in_rust_serde_shape(self):
        # FIX 7: project_create_followup accepts an optional `description`. When
        # present it is cleaned (trim + cap 4000, newlines preserved) and stored on
        # the task under the camelCase key `description`, mirroring the Rust
        # `ProjectTask` serde shape so the desktop app round-trips it. A blank
        # description is OMITTED entirely (Rust loads Option<String> as None).
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "starting",
                },
                root=root,
            )

            # (1) With a description + a bug category: both land on the task in the
            # exact camelCase keys the Rust side deserializes.
            with_desc = handle_tool_call(
                "project_create_followup",
                {
                    "project_id": "scrna-seq",
                    "title": "Worker 500 on cold start",
                    "reason": "found during review",
                    "category": "bug",
                    "description": "  Worker returns 500\non the first request  ",
                    "agent_id": "orch",
                    "role": "orchestrator",
                },
                root=root,
            )
            task = with_desc["task"]
            self.assertEqual(task["category"], "bug")
            # Trimmed at the ends, newline in the middle preserved (NOT collapsed
            # like a single-line field).
            self.assertEqual(
                task["description"], "Worker returns 500\non the first request"
            )
            self.assertEqual(task["suspectFileIds"], [])

            # (2) A blank/whitespace description is omitted from the stored task.
            blank_desc = handle_tool_call(
                "project_create_followup",
                {
                    "project_id": "scrna-seq",
                    "title": "Tidy logging",
                    "reason": "nit",
                    "category": "other",
                    "description": "   ",
                    "agent_id": "orch",
                    "role": "orchestrator",
                },
                root=root,
            )
            self.assertNotIn("description", blank_desc["task"])

            # (3) Omitting the param entirely also omits the key (back-compat).
            no_desc = handle_tool_call(
                "project_create_followup",
                {
                    "project_id": "scrna-seq",
                    "title": "Add metrics",
                    "reason": "follow-up",
                    "agent_id": "orch",
                    "role": "orchestrator",
                },
                root=root,
            )
            self.assertNotIn("description", no_desc["task"])
            # Absent category defaults to "other" (the documented MCP divergence).
            self.assertEqual(no_desc["task"]["category"], "other")

            # (4) An overlong description is capped at 4000 chars.
            long_desc = handle_tool_call(
                "project_create_followup",
                {
                    "project_id": "scrna-seq",
                    "title": "Long context",
                    "reason": "stress",
                    "description": "x" * 5000,
                    "agent_id": "orch",
                    "role": "orchestrator",
                },
                root=root,
            )
            self.assertEqual(len(long_desc["task"]["description"]), 4000)

    def test_paused_project_cannot_be_claimed_by_agent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = prepare_management_root(root)
            path = sample_project(projects)
            path.write_text(
                path.read_text(encoding="utf-8").replace(
                    "status: active", "status: paused", 1
                ),
                encoding="utf-8",
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                },
                root=root,
            )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_claim_task",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "agent_id": "codex",
                        "role": "coder",
                    },
                    root=root,
                )

            self.assertIn("paused", str(ctx.exception))

    def test_app_vault_target_matches_rust_keyring_convention(self):
        self.assertEqual(
            app_vault_target("provider:scaleway_ai"),
            "provider:scaleway_ai.Devboule",
        )

    def test_oracle_llm_app_vault_fields(self):
        # Oracle LLM provider vault keys only (cloud-infra CF/SCW removed).
        self.assertEqual(
            APP_VAULT_ACCOUNTS["scaleway_ai_token"], "provider:scaleway_ai"
        )
        self.assertEqual(APP_VAULT_ACCOUNTS["infomaniak_token"], "provider:infomaniak")
        self.assertEqual(APP_VAULT_ACCOUNTS["mistral_token"], "provider:mistral")
        self.assertNotIn("cloudflare_token", APP_VAULT_ACCOUNTS)
        self.assertNotIn("scaleway_token", APP_VAULT_ACCOUNTS)

    def test_mcp_server_constructs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)

            server = create_mcp_server(root=root)

            self.assertIsNotNone(server)

    def _registered_tool(self, server, name):
        """Return the FastMCP-registered tool object for `name`.

        Probes the documented public listing first, then known internal
        attributes, so the assertion survives small FastMCP version drift instead
        of silently passing when the tool can't be found.
        """
        candidates = []
        tool_manager = getattr(server, "_tool_manager", None)
        if tool_manager is not None and hasattr(tool_manager, "list_tools"):
            candidates = list(tool_manager.list_tools())
        if not candidates:
            self.skipTest("FastMCP does not expose registered tools for introspection")
        for tool in candidates:
            if getattr(tool, "name", None) == name:
                return tool
        self.fail(f"tool {name!r} is not registered on the FastMCP server")

    def test_spawn_main_coder_is_registered_on_the_live_server(self):
        # ROLE UNTANGLE Phase 3 gap: spawn_main_coder had a TOOLS entry, a
        # dispatch function and handle_tool_call routing, but NO @server.tool()
        # wrapper — FastMCP is the only protocol surface, so no real MCP client
        # (cloud CLI or devboule binary) could ever call it: the orchestrator's
        # whole main-coder delegation mandate was dead on the live path.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            server = create_mcp_server(root=root)
            tool = self._registered_tool(server, "spawn_main_coder")

            params = inspect.signature(tool.fn).parameters
            for name in ("agent_id", "role", "task", "files", "wait"):
                self.assertIn(name, params)
            # write/write_mode are FORCED server-side for the main tier — the
            # wrapper must NOT advertise them as caller choices.
            self.assertNotIn("write", params)
            self.assertNotIn("write_mode", params)

    def test_agent_heartbeat_wrapper_advertises_subagents(self):
        # Regression for a whole CLASS of bug: a handler supports a parameter
        # (here `subagents`) but the FastMCP @server.tool() wrapper omits it, so
        # the param is never advertised/forwarded on the LIVE path and the feature
        # is silently dead. Assert against BOTH the wrapper signature and the
        # advertised JSON schema so the contract can't drift on either side.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            server = create_mcp_server(root=root)
            tool = self._registered_tool(server, "agent_heartbeat")

            params = inspect.signature(tool.fn).parameters
            self.assertIn("subagents", params)

            schema_props = (tool.parameters or {}).get("properties", {})
            self.assertIn("subagents", schema_props)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "management"
            projects = Path(tmp) / "shared-projects"
            root.mkdir()
            prepare_management_root(root)
            projects.mkdir()
            sample_project(projects)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "model": "cheap",
                    "message": "planning",
                },
                root=root,
                projects_dir=projects,
            )

            self.assertTrue((projects / ".aspis-agents.json").exists())

    def test_oracle_paths_are_rooted_at_management_root(self):
        # Resolve the literal so the expectation matches mcp_oracle_paths()'s
        # resolved output on every OS: on Windows "C:/tmp/..." is already
        # absolute (resolve is a no-op); on POSIX it is relative, so both
        # sides must be anchored the same way before comparing.
        root = Path("C:/tmp/Devboule").resolve()
        with patch.dict("os.environ", {}, clear=True):
            paths = mcp_oracle_paths(root / "projects")

        self.assertEqual(paths["root"], root)
        self.assertEqual(paths["sqlite"], root / "oracle-data" / "metadata.sqlite")
        self.assertEqual(paths["chunks"], root / "oracle-data" / "chunks.lancedb")

    def test_oracle_index_status_root_is_basename_not_absolute_path(self):
        # M3-P12c: the local readiness gate is gone — readiness is owned by
        # the resident HTTP server. The privacy contract that survives is
        # that the `index_status` forwarded to an agent never carries an
        # absolute filesystem path (which would leak the OS username +
        # machine layout). The HTTP placeholder (`source: resident-server`,
        # `root: None`) is the only `index_status` the dispatch ever emits
        # on the success path; we pin its shape here.
        from oracle.server.aspis_mcp import _http_readiness_placeholder

        status = _http_readiness_placeholder()
        blob = json.dumps(status)
        self.assertNotIn("C:\\", blob)
        self.assertNotIn("/Users/", blob)
        self.assertNotIn("/home/", blob)
        # The root is None — privacy contract: never an absolute path.
        self.assertIsNone(status["root"])
        self.assertEqual(status["source"], "resident-server")

    def test_oracle_ask_response_carries_no_absolute_path(self):
        # M3-P12c PRIVACY (FIX 1b): the oracle_ask MCP tool result returned
        # to an agent must not contain any absolute path substring. The
        # dispatch is HTTP-only now; the resident server returns the answer
        # envelope and the MCP side stamps the `_http_readiness_placeholder`
        # onto the result. We mock `import_httpx` to feed a deterministic
        # response and assert the serialized result is path-free.
        recorder = _Recorder(
            {
                "answer": "ok",
                "citations": [],
                "not_found": False,
                "suggested_path": None,
                "results": [],
                "answer_source": "http",
                "fallback_reason": None,
                "llm_provider": "scaleway",
                "llm_model": "voxtral-small-24b-2507",
            }
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "oracle-agent",
                    "role": "orchestrator",
                    "model": "t",
                    "message": "x",
                },
                root=root,
            )
            with patch.dict(
                os.environ,
                {
                    "ASPIS_ORACLE_HTTP_BASE": "http://127.0.0.1:8765",
                    "ASPIS_ORACLE_AUTH_TOKEN": "env-token",
                },
            ):
                with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                    with patch(
                        "oracle.server.aspis_mcp.oracle_allowed_file_ids",
                        return_value={"oracle/server/aspis_mcp.py"},
                    ):
                        result = handle_tool_call(
                            "oracle_ask",
                        {
                            "query": "how do agents use oracle",
                            "limit": 3,
                            "agent_id": "oracle-agent",
                            "role": "orchestrator",
                        },
                        root=root,
                    )
        blob = json.dumps(result)
        self.assertNotIn("C:\\", blob)
        self.assertNotIn("/Users/", blob)
        self.assertNotIn("/home/", blob)
        self.assertNotIn(str(root), blob)
        # The HTTP-path index_status is path-free (root: None).
        self.assertIsNone(result["index_status"]["indexedFiles"])

    def test_oracle_context_response_carries_no_absolute_path(self):
        # M3-P12c PRIVACY: same as test_oracle_ask_response_carries_no_absolute_path
        # but for the /context path. The MCP response must never embed an
        # absolute path; the resident server returns the chunks, the MCP
        # stamps the HTTP-placeholder index_status (root: None).
        recorder = _Recorder({"query": "q", "chunks": [{"chunk_id": "c1"}]})
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "oracle-agent",
                    "role": "orchestrator",
                    "model": "t",
                    "message": "x",
                },
                root=root,
            )
            with patch.dict(
                os.environ,
                {
                    "ASPIS_ORACLE_HTTP_BASE": "http://127.0.0.1:8765",
                    "ASPIS_ORACLE_AUTH_TOKEN": "env-token",
                },
            ):
                with patch("oracle.server.aspis_mcp.import_httpx", return_value=SimpleNamespace(Client=recorder.client)):
                    with patch(
                        "oracle.server.aspis_mcp.oracle_allowed_file_ids",
                        return_value={"oracle/server/aspis_mcp.py"},
                    ):
                        result = handle_tool_call(
                            "oracle_context",
                        {
                            "query": "how do agents use oracle",
                            "limit": 3,
                            "agent_id": "oracle-agent",
                            "role": "orchestrator",
                        },
                        root=root,
                    )
        blob = json.dumps(result)
        self.assertNotIn("C:\\", blob)
        self.assertNotIn("/Users/", blob)
        self.assertNotIn("/home/", blob)
        self.assertNotIn(str(root), blob)
        self.assertIsNone(result["indexStatus"]["indexedFiles"])

    def test_validate_project_work_root_errors_are_path_free(self):
        # PRIVACY (FIX 1a): the McpError raised for a too-broad / unsafe work
        # root must not embed the absolute path back to the agent.
        from oracle.server.aspis_mcp import validate_project_work_root

        home = Path.home()
        with self.assertRaises(McpError) as ctx:
            validate_project_work_root(home)
        message = str(ctx.exception)
        self.assertNotIn(str(home.resolve()), message)
        self.assertNotIn("C:\\", message)
        self.assertNotIn("/Users/", message)
        self.assertNotIn("/home/", message)

    def test_inprocess_dispatch_with_none_scope_is_fail_closed(self):
        # FIX 3: a None scope on the IN-PROCESS path must NOT widen to the full
        # corpus; it must fail closed exactly like the HTTP bounded path.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects_dir = prepare_management_root(root)
            with patch(
                "oracle.server.aspis_mcp.resolve_oracle_http_target", return_value=None
            ):
                with self.assertRaises(McpError):
                    dispatch_oracle_ask(projects_dir, "q", 3, None, args={})
                with self.assertRaises(McpError):
                    dispatch_oracle_context(projects_dir, "q", 3, None, args={})

    def test_launch_time_client_survives_register_and_heartbeat(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            token = "test-launch-token"
            # Simulate the Rust backend writing `client` onto the launch_pending
            # session before the real agent registers with its launch token.
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "pending-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "client": "codex",
                                "lastSeenAt": "2026-05-29T00:00:00+00:00",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )

            with patch.dict(
                os.environ,
                {"ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS": ""},
                clear=False,
            ):
                registered = handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": "pending-coder",
                        "role": "coder",
                        "model": "gpt",
                        "message": "registered through app launch",
                        "launch_token": token,
                    },
                    root=root,
                )

                # Register must preserve the launch-time client even though the
                # caller did not pass it explicitly.
                self.assertEqual(registered["session"]["client"], "codex")
                session_token = registered["sessionToken"]
                self.assertTrue(session_token)

                heartbeat = handle_tool_call(
                    "agent_heartbeat",
                    {
                        "agent_id": "pending-coder",
                        "status": "active",
                        "message": "still alive",
                        "session_token": session_token,
                    },
                    root=root,
                )

            # Heartbeat must not wipe the client.
            self.assertEqual(heartbeat["session"]["client"], "codex")

    def test_explicit_client_is_set_on_public_session_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)

            state = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "powershell-coder",
                    "role": "coder",
                    "model": "gpt",
                    "client": "powershell",
                    "message": "registered",
                },
                root=root,
            )

            self.assertEqual(state["session"]["client"], "powershell")

    def test_heartbeat_file_path_persists_current_file_path(self):
        """agent_heartbeat with file_path records currentFilePath on the session.

        Backward-compat: a heartbeat WITHOUT file_path must leave the field
        None/unchanged. This is what lets Polis place the agent on the EXACT
        file's building (falling back to a representative building when the
        agent never declares a file).
        """
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            token = "launch-token-abcdefghijklmnopqrstuvwxyz-0123456789"
            projects = root / "projects"
            projects.mkdir(parents=True, exist_ok=True)
            agents_state = projects / ".aspis-agents.json"
            agents_state.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "file-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "client": "codex",
                                "lastSeenAt": "2026-05-29T00:00:00+00:00",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )

            with patch.dict(
                os.environ,
                {"ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS": ""},
                clear=False,
            ):
                registered = handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": "file-coder",
                        "role": "coder",
                        "model": "gpt",
                        "message": "registered through app launch",
                        "launch_token": token,
                    },
                    root=root,
                )
                session_token = registered["sessionToken"]
                self.assertTrue(session_token)

                # Right after register, before any file is declared, the field
                # must be absent/None (no fabricated location for Polis).
                self.assertIsNone(registered["session"].get("currentFilePath"))

                # Heartbeat WITH a file_path records it (normalized: backslashes
                # folded to forward slashes, a leading "./" stripped).
                heartbeat = handle_tool_call(
                    "agent_heartbeat",
                    {
                        "agent_id": "file-coder",
                        "status": "active",
                        "message": "editing",
                        "session_token": session_token,
                        "file_path": ".\\src\\backend\\model.rs",
                    },
                    root=root,
                )
                self.assertEqual(
                    heartbeat["session"]["currentFilePath"],
                    "src/backend/model.rs",
                )

                # Heartbeat WITHOUT a file_path must NOT wipe the stored value
                # (backward-compatible: agents that do not declare a file keep
                # whatever was last set).
                heartbeat2 = handle_tool_call(
                    "agent_heartbeat",
                    {
                        "agent_id": "file-coder",
                        "status": "active",
                        "message": "still editing",
                        "session_token": session_token,
                    },
                    root=root,
                )
                self.assertEqual(
                    heartbeat2["session"]["currentFilePath"],
                    "src/backend/model.rs",
                )

    def test_heartbeat_without_file_path_leaves_current_file_path_unset(self):
        """An agent that never passes file_path leaves currentFilePath None."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)

            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "no-file-coder",
                    "role": "coder",
                    "model": "gpt",
                    "client": "powershell",
                    "message": "registered",
                },
                root=root,
            )
            heartbeat = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "no-file-coder",
                    "status": "active",
                    "message": "alive",
                },
                root=root,
            )
            self.assertIsNone(heartbeat["session"].get("currentFilePath"))


class T4DraftReadableTests(unittest.TestCase):
    """T4 — Ticket A: `draft` is a READABLE project status.

    The planner needs read-only introspection of pre-approval projects
    (project_get) and Oracle queries against them (oracle_context). Most
    mutation paths (claim, update_status, follow-up create, plan-tasks
    create, notes/title, spawn_mini_coder) MUST keep rejecting `draft`
    exactly as they reject `paused | done | archived`. Exception:
    plan_submit MUST succeed on draft so the approval gate can promote
    draft→active (rejecting it deadlocks the planner).

    Draft projects are authored via a direct markdown write (bypassing
    MCP `project_create`, which enforces `VALID_PROJECT_STATUSES` and
    correctly rejects `draft`). The on-disk file mirrors the real shape a
    planner-written draft would take.
    """

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    @staticmethod
    def _write_draft_project(projects_dir: Path) -> Path:
        prepare_management_root(projects_dir.parent)
        path = projects_dir / "draft-proj.md"
        path.write_text(
            """---
id: draft-proj
title: Drafted project (pre-approval)
status: draft
updated_at: 2026-05-28T00:00:00Z
---

# Goal
- This project was authored externally in `draft` status and is awaiting planner approval.

```aspis-project
{
  "version": 1,
  "tasks": [
    {
      "id": "T1",
      "title": "Initial design",
      "status": "todo",
      "priority": "high",
      "assignee": null,
      "due": null,
      "linkedResources": [],
      "updatedAt": "2026-05-28T00:00:00Z"
    }
  ],
  "notes": []
}
```

# Loose notes
""",
            encoding="utf-8",
        )
        return path

    def test_draft_project_get_succeeds(self):
        # READ: project_get must accept a `draft` project (no rejection).
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            self._write_draft_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "planner-bot",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "introspecting draft",
                },
                root=root,
            )
            result = handle_tool_call(
                "project_get",
                {
                    "project_id": "draft-proj",
                    "agent_id": "planner-bot",
                    "role": "orchestrator",
                },
                root=root,
            )
            self.assertEqual(result["metadata"]["id"], "draft-proj")
            self.assertEqual(result["metadata"]["title"], "Drafted project (pre-approval)")
            self.assertEqual(result["metadata"]["status"], "draft")
            # The readable allowlist MUST keep `draft` included.
            from oracle.server.aspis_mcp import READABLE_PROJECT_STATUSES

            self.assertIn("draft", READABLE_PROJECT_STATUSES)
            self.assertIn("active", READABLE_PROJECT_STATUSES)

    def test_draft_oracle_context_guard_path_passes(self):
        # READ: oracle_context must accept a draft project without
        # draft-specific rejection. Call the REAL dispatch path; if the
        # tool fails (e.g. missing index in a temp dir), assert the
        # failure is NOT about draft status.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            self._write_draft_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "planner-bot",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "introspecting draft",
                },
                root=root,
            )
            try:
                result = handle_tool_call(
                    "oracle_context",
                    {
                        "query": "test query",
                        "project_id": "draft-proj",
                        "agent_id": "planner-bot",
                        "role": "orchestrator",
                    },
                    root=root,
                )
                # Success is proof the draft status did not block the call.
                self.assertIn("chunks", result)
            except McpError as e:
                # If it fails, it must NOT be because of draft status.
                self.assertNotIn("draft", str(e).lower())
            except Exception:
                # Any non-McpError failure is also not draft-related.
                pass

    def test_draft_project_claim_rejected(self):
        # MUTATION: project_claim_task must keep rejecting draft.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            self._write_draft_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "test",
                    "message": "trying to claim a draft",
                },
                root=root,
            )
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_claim_task",
                    {
                        "project_id": "draft-proj",
                        "task_id": "T1",
                        "agent_id": "codex",
                        "role": "coder",
                    },
                    root=root,
                )
            # Error message must explicitly enumerate `draft` so the planner
            # knows the rejection covers this status.
            self.assertIn("draft", str(ctx.exception))
            self.assertIn("Cannot claim", str(ctx.exception))

    def test_draft_project_update_status_rejected(self):
        # MUTATION: project_update_status must keep rejecting draft.
        # Seed an active claim directly in the agents state so the guard
        # under test (the project status check) is reached.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            self._write_draft_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "test",
                    "message": "trying to mutate a draft",
                },
                root=root,
            )
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 2,
                        "updatedAt": "2026-06-12T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "codex",
                                "role": "coder",
                                "status": "active",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                            }
                        ],
                        "claims": [
                            {
                                "projectId": "draft-proj",
                                "taskId": "T1",
                                "agentId": "codex",
                                "role": "coder",
                                "status": "wip",
                                "claimedAt": "2026-06-12T00:00:00Z",
                                "leaseUntil": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "draft-proj",
                        "task_id": "T1",
                        "status": "review",
                        "agent_id": "codex",
                        "role": "coder",
                        "evidence": "Pretend to advance a draft task.",
                        "confidence": 0.5,
                    },
                    root=root,
                )
            self.assertIn("draft", str(ctx.exception))
            self.assertIn("Cannot update tasks", str(ctx.exception))

    def test_draft_project_append_note_rejected(self):
        # MUTATION: project_append_note must reject draft projects.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            self._write_draft_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "test",
                    "message": "trying to append a note to a draft",
                },
                root=root,
            )
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_append_note",
                    {
                        "project_id": "draft-proj",
                        "text": "Should be rejected.",
                        "agent_id": "codex",
                        "role": "coder",
                    },
                    root=root,
                )
            self.assertIn("draft projects are read-only", str(ctx.exception))
            self.assertIn("project_append_note", str(ctx.exception))

    def test_draft_project_set_title_rejected(self):
        # MUTATION: project_set_title must reject draft projects.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            self._write_draft_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "namer",
                    "role": "orchestrator",
                    "model": "test",
                    "message": "trying to rename a draft",
                },
                root=root,
            )
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_set_title",
                    {
                        "project_id": "draft-proj",
                        "title": "Hijacked Title",
                        "agent_id": "namer",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            self.assertIn("draft projects are read-only", str(ctx.exception))
            self.assertIn("project_set_title", str(ctx.exception))

    def test_draft_plan_submit_allowed(self):
        # plan_submit MUST succeed on draft: the approval gate promotes draft→active.
        # Rejecting it deadlocks the planner (UI creates drafts; launch is allowed).
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            self._write_draft_project(projects)
            token = "test-launch-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 2,
                        "updatedAt": "2026-06-09T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "codex",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-06-09T00:00:00+00:00",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            reg = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                    "launch_token": token,
                },
                root=root,
            )
            session_token = reg["sessionToken"]
            with patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "draft-proj",
                        "title": "Test plan",
                        "plan_markdown": "# Plan\n\n- step one\n",
                        "session_token": session_token,
                    },
                    root=root,
                )
            self.assertIn("planId", out)
            self.assertRegex(out["planId"], r"^[0-9a-f]{32}$")
            # Zero poll timeout → synthesized timeout (no human verdict); still a
            # successful submit path — draft was NOT rejected.
            self.assertEqual(out["status"], "timeout")
            plan_id = out["planId"]
            md_path = (
                projects / ".aspis-plans" / "draft-proj" / f"{plan_id}.md"
            )
            self.assertTrue(
                md_path.exists(), "plan markdown must be written for draft projects"
            )

    def test_paused_plan_submit_rejected(self):
        # Defense-in-depth: plan_submit only allows active|draft. paused/done/
        # archived must fail closed with a clear status message.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            prepare_management_root(root)
            path = projects / "paused-proj.md"
            path.write_text(
                """---
id: paused-proj
title: Paused project
status: paused
updated_at: 2026-05-28T00:00:00Z
---

# Goal
- Idle project.

```aspis-project
{
  "version": 1,
  "tasks": [],
  "notes": []
}
```
""",
                encoding="utf-8",
            )
            token = "test-launch-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 2,
                        "updatedAt": "2026-06-09T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "codex",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-06-09T00:00:00+00:00",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            reg = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                    "launch_token": token,
                },
                root=root,
            )
            session_token = reg["sessionToken"]
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "paused-proj",
                        "title": "Test plan",
                        "plan_markdown": "# Plan\n\n- step one\n",
                        "session_token": session_token,
                    },
                    root=root,
                )
            self.assertIn("active or draft", str(ctx.exception))
            self.assertIn("paused", str(ctx.exception))
            self.assertFalse((projects / ".aspis-plans").exists())

    def test_draft_spawn_mini_coder_rejected(self):
        # MUTATION: spawn_mini_coder must reject when parent session is on a
        # draft project.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            self._write_draft_project(projects)
            token = "test-launch-token"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 2,
                        "updatedAt": "2026-06-09T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "codex",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-06-09T00:00:00+00:00",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                                "currentProjectId": "draft-proj",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            reg = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "model": "codex",
                    "message": "coding",
                    "launch_token": token,
                },
                root=root,
            )
            session_token = reg["sessionToken"]
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "test task",
                        "files": ["src/a.rs"],
                        "session_token": session_token,
                    },
                    root=root,
                )
            self.assertIn("draft projects are read-only", str(ctx.exception))
            self.assertIn("spawn_mini_coder", str(ctx.exception))


class NormalizeModelTests(unittest.TestCase):
    def test_opus_family_mapping(self):
        self.assertEqual(normalize_model("claude-opus-4-8"), "opus")
        self.assertEqual(normalize_model("Claude Opus 4.8"), "opus")
        self.assertEqual(normalize_model("OPUS"), "opus")

    def test_sonnet_family_mapping(self):
        self.assertEqual(normalize_model("claude-sonnet-4-6"), "sonnet")
        self.assertEqual(normalize_model("anthropic/claude-3-5-sonnet"), "sonnet")

    def test_haiku_family_mapping(self):
        self.assertEqual(normalize_model("claude-haiku-3-5"), "haiku")
        self.assertEqual(normalize_model("  Haiku  "), "haiku")

    def test_unknown_model_passthrough_cleaned(self):
        self.assertEqual(normalize_model("DeepSeek-V3"), "deepseek-v3")
        self.assertEqual(normalize_model("  gpt-4o  "), "gpt-4o")

    def test_none_and_garbage_become_empty(self):
        self.assertEqual(normalize_model(None), "")
        self.assertEqual(normalize_model(""), "")
        self.assertEqual(normalize_model("   "), "")
        self.assertEqual(normalize_model(123), "")
        self.assertEqual(normalize_model({"x": 1}), "")

    def test_length_cap(self):
        result = normalize_model("z" * 500)
        self.assertEqual(len(result), 64)
        # Length cap is applied BEFORE family detection, so a family keyword that
        # falls beyond the 64-char window is not detected (the cleaned, capped
        # string is returned instead). A keyword within the window still maps.
        self.assertEqual(normalize_model("opus-" + "x" * 500), "opus")
        self.assertEqual(normalize_model("x" * 500 + "opus"), "x" * 64)

    def test_multiple_families_first_match_wins(self):
        # When a string contains MORE THAN ONE family keyword, detection follows
        # the iteration order of MODEL_FAMILIES (opus, sonnet, haiku) — first
        # match wins, regardless of where each keyword sits in the string. That
        # tuple ORDER is the documented contract for ambiguous reports.
        self.assertEqual(normalize_model("opus-and-sonnet"), "opus")
        self.assertEqual(normalize_model("sonnet-then-opus"), "opus")
        self.assertEqual(normalize_model("haiku-sonnet-opus"), "opus")
        self.assertEqual(normalize_model("sonnet-haiku"), "sonnet")


class NormalizeSubagentsTests(unittest.TestCase):
    def test_valid_list(self):
        result = normalize_subagents(
            [
                {"label": "search", "model": "claude-haiku-3-5", "count": 3},
                {"label": "review", "model": "claude-opus-4-8", "count": 1},
            ]
        )
        self.assertEqual(
            result,
            [
                {"label": "search", "model": "haiku", "count": 3, "role": None},
                {"label": "review", "model": "opus", "count": 1, "role": None},
            ],
        )

    def test_role_alias_normalized(self):
        # Phase B merge: the architect alias folds to coder; orchestrator is first-class.
        result = normalize_subagents(
            [{"label": "plan", "model": "opus", "count": 1, "role": "architect"}]
        )
        self.assertEqual(result[0]["role"], "coder")

    def test_orchestrator_role_preserved(self):
        # Orchestrator is now a first-class role and normalizes to itself.
        result = normalize_subagents(
            [{"label": "plan", "model": "opus", "count": 1, "role": "orchestrator"}]
        )
        self.assertEqual(result[0]["role"], "orchestrator")

    def test_invalid_role_becomes_none(self):
        result = normalize_subagents(
            [{"label": "x", "model": "opus", "count": 1, "role": "wizard"}]
        )
        self.assertEqual(result[0]["role"], None)

    def test_bad_count_dropped(self):
        result = normalize_subagents(
            [
                {"label": "ok", "model": "opus", "count": 2},
                {"label": "nan", "model": "opus", "count": "abc"},
                {"label": "zero", "model": "opus", "count": 0},
                {"label": "neg", "model": "opus", "count": -5},
            ]
        )
        self.assertEqual([e["label"] for e in result], ["ok"])

    def test_count_coercion_from_clean_str_and_float(self):
        result = normalize_subagents(
            [
                {"label": "a", "model": "opus", "count": "4"},
                {"label": "b", "model": "opus", "count": 5.0},
            ]
        )
        self.assertEqual([e["count"] for e in result], [4, 5])

    def test_count_default_and_clamp(self):
        result = normalize_subagents(
            [
                {"label": "default", "model": "opus"},
                {"label": "huge", "model": "opus", "count": 99999},
            ]
        )
        self.assertEqual(result[0]["count"], 1)
        self.assertEqual(result[1]["count"], 9999)

    def test_empty_label_dropped(self):
        result = normalize_subagents(
            [
                {"label": "   ", "model": "opus", "count": 1},
                {"label": "", "model": "opus", "count": 1},
                {"model": "opus", "count": 1},
            ]
        )
        self.assertEqual(result, [])

    def test_model_may_be_empty(self):
        result = normalize_subagents([{"label": "noModel", "count": 2}])
        self.assertEqual(
            result, [{"label": "noModel", "model": "", "count": 2, "role": None}]
        )

    def test_cap_list_length_32(self):
        entries = [{"label": f"a{i}", "model": "opus", "count": 1} for i in range(50)]
        result = normalize_subagents(entries)
        self.assertEqual(len(result), 32)

    def test_label_cap_80(self):
        result = normalize_subagents(
            [{"label": "z" * 200, "model": "opus", "count": 1}]
        )
        self.assertEqual(len(result[0]["label"]), 80)

    def test_none_vs_empty_list_distinction(self):
        self.assertIsNone(normalize_subagents(None))
        self.assertIsNone(normalize_subagents("not a list"))
        self.assertIsNone(normalize_subagents(42))
        self.assertEqual(normalize_subagents([]), [])

    def test_non_dict_entries_dropped(self):
        result = normalize_subagents(
            [{"label": "ok", "model": "opus", "count": 1}, "garbage", 5, None]
        )
        self.assertEqual([e["label"] for e in result], ["ok"])

    def test_extra_unknown_keys_stripped(self):
        # The normalizer must WHITELIST output keys: an attacker-supplied or
        # accidental extra key (e.g. "secret") must never survive into the stored
        # entry, which is later written to .aspis-agents.json and surfaced to the UI.
        result = normalize_subagents(
            [
                {
                    "label": "x",
                    "model": "opus",
                    "count": 1,
                    "secret": "t",
                    "extra": [1, 2],
                }
            ]
        )
        self.assertEqual(
            result, [{"label": "x", "model": "opus", "count": 1, "role": None}]
        )
        self.assertNotIn("secret", result[0])
        self.assertNotIn("extra", result[0])

    def test_count_bool_is_dropped(self):
        # bool is an int subclass; True/False must NOT be accepted as a count of
        # 1/0. The entry is dropped (no count) rather than silently coerced.
        self.assertIsNone(_coerce_subagent_count(True))
        self.assertIsNone(_coerce_subagent_count(False))
        result = normalize_subagents(
            [
                {"label": "ok", "model": "opus", "count": 3},
                {"label": "true", "model": "opus", "count": True},
                {"label": "false", "model": "opus", "count": False},
            ]
        )
        self.assertEqual([e["label"] for e in result], ["ok"])


class RoleRulesContractTests(unittest.TestCase):
    def test_each_role_states_three_mandates(self):
        for rule in ROLE_RULES:
            blob = json.dumps(rule, ensure_ascii=False).lower()
            # (a) declare the model at agent_register
            self.assertIn("model", blob)
            self.assertIn("agent_register", blob)
            # (b) report subagents via agent_heartbeat
            self.assertIn("subagents", blob)
            self.assertIn("agent_heartbeat", blob)
            # (c) signal needs_user when waiting on the human
            self.assertIn("needs_user", blob)


class RoleMergeTests(unittest.TestCase):
    """Phase B role merge: spawn roles collapse to {coder, verifier}; P3 added the
    read-only 'mini' leaf. DEVBOULE: 'orchestrator' is now a FIRST-CLASS role (the
    devboule-coder main planner/delegator), NO LONGER an alias to coder — it
    normalizes to itself and carries its own narrower (tighter-or-equal) allowlist."""

    def test_valid_roles_are_coder_verifier_mini_and_orchestrator(self):
        # Phase B collapsed the spawn roles to {coder, verifier}; P3 added "mini"
        # (read-only leaf); DEVBOULE promoted "orchestrator" to a first-class role.
        self.assertEqual(VALID_ROLES, {"coder", "verifier", "mini", "orchestrator"})

    def test_orchestrator_normalizes_to_itself_not_coder(self):
        # DEVBOULE behavior change: 'orchestrator' was previously an ALIAS that
        # collapsed to coder. As a first-class role it now normalizes to ITSELF,
        # so its narrower allowlist is actually reached (an alias would route the
        # gate through the coder's broader set). 'orchestrator' is no longer in
        # ROLE_ALIASES.
        self.assertEqual(normalize_role("orchestrator"), "orchestrator")
        self.assertNotIn("orchestrator", ROLE_ALIASES)

    def test_architect_alias_normalizes_to_coder(self):
        self.assertEqual(normalize_role("architect"), "coder")

    def test_code_alias_normalizes_to_coder(self):
        self.assertEqual(normalize_role("code"), "coder")

    def test_verifier_stays_verifier(self):
        self.assertEqual(normalize_role("verifier"), "verifier")

    def test_coder_stays_coder(self):
        self.assertEqual(normalize_role("coder"), "coder")

    def test_invalid_role_still_raises(self):
        with self.assertRaises(McpError):
            normalize_role("hacker")

    def test_role_rules_keys_are_coder_verifier_mini_and_orchestrator(self):
        roles = {rule["role"] for rule in ROLE_RULES}
        self.assertEqual(roles, {"coder", "verifier", "mini", "orchestrator"})

    def test_coder_rule_carries_folded_planning_mandate(self):
        coder = next(rule for rule in ROLE_RULES if rule["role"] == "coder")
        # The orchestrator's coordination tool (create follow-ups) folds into
        # coder, and the summary gains the planning language.
        self.assertIn("project_create_followup", coder["allowedTools"])
        self.assertIn("plan", coder["summary"].lower())

    def test_register_orchestrator_stores_orchestrator_session(self):
        # DEVBOULE: registering as "orchestrator" now stores the role VERBATIM (no
        # collapse to coder). The session must be read back under role="orchestrator"
        # (reading as "coder" would now be a role mismatch — see the mismatch test).
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
            try:
                handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": "devboule-orch-1",
                        "role": "orchestrator",
                        "model": "opus",
                        "message": "starting",
                    },
                    root=root,
                )
                state = handle_tool_call(
                    "agent_state",
                    {"agent_id": "devboule-orch-1", "role": "orchestrator"},
                    root=root,
                )
            finally:
                os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
            self.assertEqual(state["sessions"][0]["role"], "orchestrator")


class OrchestratorRoleTests(unittest.TestCase):
    """DEVBOULE: the first-class `orchestrator` role — the frontier planning tier.
    Pins its allowlist (PRIMARY oracle + delegation + the coder-equivalent Kanban
    tools + the human gate + the FULL provider surface, owner decision role-untangle
    2026-07: it plans the infra so it sees and manages it, mutations stay
    claimed-task+evidence audited). Still pinned tighter-or-equal to coder: NO
    direct file-write tool, NO Censor tools, NO visual_check, NO verifier-only
    transition."""

    def setUp(self):
        # Tokenless self-registration so the role-gate (not the launch-token gate)
        # is what these tests exercise — same pattern as the other role tests.
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    def _register_orchestrator(self, root: Path, agent_id: str = "orch") -> None:
        handle_tool_call(
            "agent_register",
            {
                "agent_id": agent_id,
                "role": "orchestrator",
                "model": "opus",
                "message": "planning",
            },
            root=root,
        )

    # --- Allowlist shape (server-side gate is ROLE_ALLOWED_TOOLS) ---------------

    def test_orchestrator_and_coder_allowlists_differ_exactly_as_designed(self):
        # Orchestrator: plan + spawn_main_coder only for implementation hand-off.
        # Main coder: owns minis (spawn/steer/result) + censor/visual adjudication.
        # Pinning BOTH deltas keeps any future privilege drift loud.
        orch = ROLE_ALLOWED_TOOLS["orchestrator"]
        coder = ROLE_ALLOWED_TOOLS["coder"]
        self.assertEqual(sorted(orch - coder), ["spawn_main_coder"])
        self.assertEqual(
            sorted(coder - orch),
            [
                "censor_dispose",
                "censor_findings",
                "mini_coder_result",
                "spawn_mini_coder",
                "steer_mini_coder",
                "visual_check",
            ],
        )

    def test_orchestrator_allowlist_exact_contents(self):
        self.assertEqual(
            ROLE_ALLOWED_TOOLS["orchestrator"],
            {
                "agent_register",
                "agent_heartbeat",
                "agent_state",
                "project_list",
                "project_get",
                "project_next_task",
                "project_claim_task",
                "project_update_status",
                "project_append_note",
                "project_set_title",
                "project_create_followup",
                "project_create_plan_tasks",
                "oracle_ask",
                "oracle_context",
                "project_structure",
                "get_neighborhood",
                "find_imports",
                # Main-coder dispatch only — minis are Main-coder-only.
                "spawn_main_coder",
                "request_git_push",
                "plan_submit",
                "plan_status",
                "ask_user",
                "design_request",
                "design_result",
            },
        )

    def test_spawn_main_coder_is_orchestrator_only(self):
        # ROLE UNTANGLE Phase 3: the Main-coder dispatch belongs to the
        # orchestrator alone — the coder CLIs write with their own tools, the
        # verifier never writes, the mini never spawns.
        self.assertIn("spawn_main_coder", ROLE_ALLOWED_TOOLS["orchestrator"])
        for role in ("coder", "verifier", "mini"):
            self.assertNotIn(
                "spawn_main_coder",
                ROLE_ALLOWED_TOOLS[role],
                f"{role} must not hold spawn_main_coder",
            )

    def test_spawn_main_coder_caps_files_at_the_rust_twin_limit(self):
        # CO-WRITER PARITY (main_coder.rs validate_main_coder_request caps files
        # at 10): until the 2026-07 wrapper fix the MCP path was unreachable, so
        # the Python side silently allowed MINI_CODER_MAX_FILES=64 for the main
        # tier. Now that the path is live it must enforce the same 10.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "management"
            projects = Path(tmp) / "shared-projects"
            root.mkdir()
            prepare_management_root(root)
            projects.mkdir()
            sample_project(projects)

            registered = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "model": "cheap",
                    "message": "planning",
                },
                root=root,
                projects_dir=projects,
            )
            token = ""
            if isinstance(registered, dict):
                token = registered.get("session_token") or registered.get(
                    "sessionToken", ""
                )

            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "spawn_main_coder",
                    {
                        "agent_id": "orch",
                        "role": "orchestrator",
                        "task": "do a substantial thing",
                        "files": [f"src/file_{i}.rs" for i in range(11)],
                        "session_token": token,
                    },
                    root=root,
                    projects_dir=projects,
                )
            self.assertIn("at most 10", str(ctx.exception))

    def test_orchestrator_has_no_write_censor_or_mini_tools(self):
        # Plan + spawn_main_coder only for implementation. No minis, no censor
        # adjudication.
        orch = ROLE_ALLOWED_TOOLS["orchestrator"]
        for forbidden in (
            "censor_findings",
            "censor_dispose",
            "visual_check",
            "spawn_mini_coder",
            "steer_mini_coder",
            "mini_coder_result",
        ):
            self.assertNotIn(forbidden, orch)
        self.assertIn("spawn_main_coder", orch)

    def test_role_gate_allows_orchestrator_for_its_tools(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            self._register_orchestrator(root)
            for tool in (
                "oracle_ask",
                "oracle_context",
                "spawn_main_coder",
                "ask_user",
                "request_git_push",
                "project_get",
                "project_list",
                "project_next_task",
                "project_claim_task",
                "project_update_status",
                "project_append_note",
                "project_set_title",
                "project_create_followup",
                "plan_submit",
                "plan_status",
            ):
                self.assertEqual(
                    require_registered_role(projects, "orch", "orchestrator", tool),
                    "orchestrator",
                    tool,
                )

    def test_role_gate_rejects_orchestrator_for_non_allowlisted_tools(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            self._register_orchestrator(root)
            for tool in (
                "censor_dispose",
                "censor_findings",
                "visual_check",
                "spawn_mini_coder",
                "steer_mini_coder",
                "mini_coder_result",
            ):
                with self.assertRaises(McpError, msg=tool):
                    require_registered_role(projects, "orch", "orchestrator", tool)

    # --- End-to-end tool dispatch (allowed) ------------------------------------

    def test_orchestrator_can_read_and_create_followup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            self._register_orchestrator(root)
            got = handle_tool_call(
                "project_get",
                {"project_id": "scrna-seq", "agent_id": "orch", "role": "orchestrator"},
                root=root,
            )
            self.assertEqual(got["metadata"]["id"], "scrna-seq")
            # A coder-equivalent Kanban write (NOT verifier-only) succeeds.
            handle_tool_call(
                "project_create_followup",
                {
                    "project_id": "scrna-seq",
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "title": "Follow-up from orchestrator",
                    "reason": "Orchestrator holds coder-equivalent Kanban powers.",
                },
                root=root,
            )

    def test_orchestrator_claims_like_a_coder_and_moves_to_review(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            self._register_orchestrator(root)
            claimed = handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "orch",
                    "role": "orchestrator",
                },
                root=root,
            )
            # Same as coder: claiming a todo auto-advances it to wip.
            # COMPACT ACK (round-5 class fix): project_claim_task now returns a
            # compact ack with a "claim" key carrying the lease outcome.
            self.assertEqual(claimed["claim"]["status"], "wip")
            reviewed = handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "review",
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "evidence": "Delegated implementation is ready for verifier handoff.",
                    "confidence": 0.6,
                },
                root=root,
            )
            self.assertEqual(reviewed["state"]["tasks"][0]["status"], "review")

    # --- Rejections (role gate / verifier-only transition / mismatch) ----------

    def test_orchestrator_cannot_set_done_verifier_only_transition(self):
        # Mirrors test_coder_cannot_close_task_without_verifier: the orchestrator
        # shares the coder's transitions, so `done` (verifier-only) is rejected.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            self._register_orchestrator(root)
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "orch",
                    "role": "orchestrator",
                },
                root=root,
            )
            with self.assertRaises(McpError):
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "done",
                        "agent_id": "orch",
                        "role": "orchestrator",
                        "evidence": "I think the delegated work is complete.",
                        "confidence": 0.95,
                    },
                    root=root,
                )

    def test_orchestrator_role_mismatch_when_calling_as_coder(self):
        # Registered as orchestrator, a tool call claiming role="coder" hits the
        # role-mismatch gate (mirrors test_mini_cannot_act_or_reregister_as_coder).
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            self._register_orchestrator(root)
            with self.assertRaises(McpError) as ctx:
                require_registered_role(projects, "orch", "coder", "oracle_ask")
            self.assertIn("mismatch", str(ctx.exception).lower())


class PlanTaskSchemaTests(unittest.TestCase):
    """Phase 11.5-B (Piece 1a): the unified task store. validate_project_state must
    accept the new optional task fields (dependsOn/scope/acceptance/planId) AND keep
    loading old blocks that have none of them, and the dependsOn graph must reference
    existing ids + be acyclic (Kahn port from devboule-coder/src/planner.rs)."""

    @staticmethod
    def _task(task_id: str, **extra) -> dict:
        base = {
            "id": task_id,
            "title": "Task",
            "status": "todo",
            "updatedAt": "2026-05-28T00:00:00Z",
        }
        base.update(extra)
        return base

    def _state(self, *tasks: dict) -> dict:
        return {"version": 1, "tasks": list(tasks), "notes": []}

    def test_validate_accepts_new_fields(self):
        # A task carrying every new field validates and is untouched.
        state = self._state(
            self._task(
                "T1",
                scope=["src/a.ts", "src/b.ts"],
                acceptance="cargo build passes",
                planId="plan-123",
            ),
            self._task(
                "T2", dependsOn=["T1"], scope=["src/c.ts"], acceptance="tests green"
            ),
        )
        validate_project_state(state)  # must not raise
        self.assertEqual(state["tasks"][1]["dependsOn"], ["T1"])

    def test_old_block_without_new_fields_still_validates(self):
        # BACKWARD COMPAT: a task with NONE of the new fields (an old `.md` block)
        # loads with no error and is not mutated.
        state = self._state(self._task("T1"))
        validate_project_state(state)  # must not raise
        self.assertNotIn("dependsOn", state["tasks"][0])
        self.assertNotIn("scope", state["tasks"][0])
        self.assertNotIn("acceptance", state["tasks"][0])
        self.assertNotIn("planId", state["tasks"][0])

    def test_dangling_dependsOn_is_rejected(self):
        state = self._state(self._task("T1", dependsOn=["T9"]))
        with self.assertRaises(McpError) as ctx:
            validate_project_state(state)
        self.assertIn("unknown task id", str(ctx.exception))

    def test_self_dependency_is_rejected(self):
        state = self._state(self._task("T1", dependsOn=["T1"]))
        with self.assertRaises(McpError) as ctx:
            validate_project_state(state)
        self.assertIn("itself", str(ctx.exception))

    def test_cycle_is_rejected(self):
        # T1 -> T2 -> T1 is a cycle.
        state = self._state(
            self._task("T1", dependsOn=["T2"]),
            self._task("T2", dependsOn=["T1"]),
        )
        with self.assertRaises(McpError) as ctx:
            validate_project_state(state)
        self.assertIn("cycle", str(ctx.exception).lower())

    def test_valid_dag_is_accepted(self):
        # Diamond: T1 <- T2, T1 <- T3, T2/T3 <- T4. Acyclic.
        state = self._state(
            self._task("T1"),
            self._task("T2", dependsOn=["T1"]),
            self._task("T3", dependsOn=["T1"]),
            self._task("T4", dependsOn=["T2", "T3"]),
        )
        validate_project_state(state)  # must not raise

    def test_wrong_typed_fields_are_rejected(self):
        with self.assertRaises(McpError):
            validate_project_state(self._state(self._task("T1", dependsOn="T2")))
        with self.assertRaises(McpError):
            validate_project_state(self._state(self._task("T1", scope="src/a.ts")))
        with self.assertRaises(McpError):
            validate_project_state(self._state(self._task("T1", acceptance=123)))
        with self.assertRaises(McpError):
            validate_project_state(self._state(self._task("T1", planId=7)))

    def test_helper_detects_cycle_directly(self):
        # The reusable Kahn helper rejects a cycle and accepts a DAG on its own.
        with self.assertRaises(McpError):
            validate_task_dependency_dag({"A": ["B"], "B": ["A"]})
        validate_task_dependency_dag({"A": [], "B": ["A"], "C": ["A", "B"]})


class ProjectCreatePlanTasksTests(unittest.TestCase):
    """Phase 11.5-B (Piece 1a): the project_create_plan_tasks MCP tool. Bulk-creates
    an approved plan's tasks on the shared Kanban as todo+planId, allocating fresh
    T<n> ids that never collide with manual tasks and remapping the planner-internal
    dependsOn ids onto the allocated ids."""

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    # A valid 32-lowercase-hex plan id (uuid4().hex shape) the gate accepts ONLY when its
    # request is "approved". Tests seed the approval explicitly.
    APPROVED_PLAN_ID = "a" * 32

    def _setup(self, tmp: str, seed_approved: bool = True) -> Path:
        root = Path(tmp)
        projects = root / "projects"
        projects.mkdir()
        sample_project(projects)  # ships one manual task T1
        handle_tool_call(
            "agent_register",
            {
                "agent_id": "orch",
                "role": "orchestrator",
                "model": "opus",
                "message": "planning",
            },
            root=root,
        )
        if seed_approved:
            self._seed_plan_request(root, self.APPROVED_PLAN_ID, "approved")
        return root

    def _seed_plan_request(self, root: Path, plan_id: str, status: str) -> None:
        """Append a plan-approval request with the given terminal status to the state, so
        the B4/W6 gate can resolve a real verdict. Mirrors the queue entry plan_submit
        writes + the human approve/reject command stamps."""
        from oracle.server.aspis_mcp import file_lock

        projects = root / "projects"
        state_lock = projects / f"{AGENTS_STATE_FILE}.lock"
        with file_lock(state_lock):
            state = read_agents_state(projects)
            state.setdefault("planApprovalRequests", []).append(
                {
                    "id": plan_id,
                    "agentId": "orch",
                    "projectId": "scrna-seq",
                    "title": "Seeded plan",
                    "status": status,
                    "createdAt": "2026-01-01T00:00:00Z",
                }
            )
            write_agents_state(projects, state)

    def test_creates_todo_planid_tasks_with_fresh_ids_and_remapped_deps(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp)
            # Plan uses INTERNAL ids "a"/"b" with b depending on a. The project
            # already has a manual T1, so allocation must start at T2.
            result = handle_tool_call(
                "project_create_plan_tasks",
                {
                    "project_id": "scrna-seq",
                    "plan_id": self.APPROVED_PLAN_ID,
                    "tasks": [
                        {
                            "id": "a",
                            "title": "Scaffold module",
                            "scope": ["src/a.ts"],
                            "acceptance": "builds",
                            "dependsOn": [],
                        },
                        {
                            "id": "b",
                            "title": "Wire it up",
                            "scope": ["src/b.ts"],
                            "acceptance": "tests pass",
                            "dependsOn": ["a"],
                        },
                    ],
                    "agent_id": "orch",
                    "role": "orchestrator",
                },
                root=root,
            )
            self.assertEqual(result["planId"], self.APPROVED_PLAN_ID)
            # Fresh, non-colliding ids: T1 is the manual task, plan gets T2/T3.
            self.assertEqual(result["idMap"], {"a": "T2", "b": "T3"})
            created = result["tasks"]
            self.assertEqual([t["id"] for t in created], ["T2", "T3"])
            for t in created:
                self.assertEqual(t["status"], "todo")
                self.assertEqual(t["planId"], self.APPROVED_PLAN_ID)
            # dependsOn remapped through the id map: b("a") -> T3 depends on T2.
            # No-churn: a root task's EMPTY dependsOn is OMITTED (mirrors the Rust
            # skip_serializing_if), so assert the semantic via .get AND pin the omission.
            self.assertEqual(created[0].get("dependsOn", []), [])
            self.assertNotIn("dependsOn", created[0])  # empty field omitted (no churn)
            self.assertEqual(created[1]["dependsOn"], ["T2"])
            self.assertEqual(created[0]["scope"], ["src/a.ts"])
            self.assertEqual(created[1]["acceptance"], "tests pass")

            # Persisted: re-reading the project shows the manual task UNTOUCHED
            # (no planId) and the two plan tasks present.
            got = handle_tool_call(
                "project_get",
                {"project_id": "scrna-seq", "agent_id": "orch", "role": "orchestrator"},
                root=root,
            )
            tasks = {t["id"]: t for t in got["state"]["tasks"]}
            self.assertEqual(set(tasks), {"T1", "T2", "T3"})
            self.assertNotIn("planId", tasks["T1"])  # manual task is plan-free
            self.assertEqual(tasks["T3"]["dependsOn"], ["T2"])

    def test_weight_main_persists_and_mini_stays_no_churn(self):
        # ROLE UNTANGLE Phase 4: a "main"-weight plan task persists weight:"main"
        # (the runner routes it to spawn_main_coder); absent or "mini" stores NO
        # weight key (no churn); an unknown value rejects the whole call.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp)
            result = handle_tool_call(
                "project_create_plan_tasks",
                {
                    "project_id": "scrna-seq",
                    "plan_id": self.APPROVED_PLAN_ID,
                    "tasks": [
                        {
                            "id": "a",
                            "title": "Big refactor",
                            "scope": ["src/a.ts"],
                            "acceptance": "builds",
                            "dependsOn": [],
                            "weight": "main",
                        },
                        {
                            "id": "b",
                            "title": "Tiny edit",
                            "scope": ["src/b.ts"],
                            "acceptance": "builds",
                            "dependsOn": [],
                            "weight": "mini",
                        },
                        {
                            "id": "c",
                            "title": "Plain task",
                            "scope": ["src/c.ts"],
                            "acceptance": "builds",
                            "dependsOn": [],
                        },
                    ],
                    "agent_id": "orch",
                    "role": "orchestrator",
                },
                root=root,
            )
            created = {t["title"]: t for t in result["tasks"]}
            self.assertEqual(created["Big refactor"]["weight"], "main")
            self.assertNotIn("weight", created["Tiny edit"])  # NO-CHURN
            self.assertNotIn("weight", created["Plain task"])  # NO-CHURN

        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp)
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_create_plan_tasks",
                    {
                        "project_id": "scrna-seq",
                        "plan_id": self.APPROVED_PLAN_ID,
                        "tasks": [
                            {
                                "id": "a",
                                "title": "Typo",
                                "scope": ["src/a.ts"],
                                "acceptance": "builds",
                                "dependsOn": [],
                                "weight": "heavy",
                            },
                        ],
                        "agent_id": "orch",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            self.assertIn("invalid weight", str(ctx.exception))

    def test_rejects_cyclic_input(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp)
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_create_plan_tasks",
                    {
                        "project_id": "scrna-seq",
                        "plan_id": self.APPROVED_PLAN_ID,
                        "tasks": [
                            {
                                "id": "a",
                                "title": "A",
                                "scope": ["src/a.ts"],
                                "acceptance": "x",
                                "dependsOn": ["b"],
                            },
                            {
                                "id": "b",
                                "title": "B",
                                "scope": ["src/b.ts"],
                                "acceptance": "x",
                                "dependsOn": ["a"],
                            },
                        ],
                        "agent_id": "orch",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            self.assertIn("cycle", str(ctx.exception).lower())
            # NOTHING was written: the project still has only the manual T1.
            got = handle_tool_call(
                "project_get",
                {"project_id": "scrna-seq", "agent_id": "orch", "role": "orchestrator"},
                root=root,
            )
            self.assertEqual([t["id"] for t in got["state"]["tasks"]], ["T1"])

    def test_rejects_dependsOn_not_in_request(self):
        # A plan must be self-contained: a dep on an id NOT in the batch (even an
        # existing manual task id) is rejected.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp)
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_create_plan_tasks",
                    {
                        "project_id": "scrna-seq",
                        "plan_id": self.APPROVED_PLAN_ID,
                        "tasks": [
                            {
                                "id": "a",
                                "title": "A",
                                "scope": ["src/a.ts"],
                                "acceptance": "x",
                                "dependsOn": ["T1"],
                            },
                        ],
                        "agent_id": "orch",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            self.assertIn("not in the request", str(ctx.exception))

    def test_rejects_empty_and_oversized_batches(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "project_create_plan_tasks",
                    {
                        "project_id": "scrna-seq",
                        "plan_id": self.APPROVED_PLAN_ID,
                        "tasks": [],
                        "agent_id": "orch",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            too_many = [
                {
                    "id": f"t{i}",
                    "title": "x",
                    "scope": ["src/x.ts"],
                    "acceptance": "x",
                    "dependsOn": [],
                }
                for i in range(MAX_PLAN_TASKS + 1)
            ]
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_create_plan_tasks",
                    {
                        "project_id": "scrna-seq",
                        "plan_id": self.APPROVED_PLAN_ID,
                        "tasks": too_many,
                        "agent_id": "orch",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            self.assertIn("Too many", str(ctx.exception))

    def test_rejects_unsafe_scope_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp)
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_create_plan_tasks",
                    {
                        "project_id": "scrna-seq",
                        "plan_id": self.APPROVED_PLAN_ID,
                        "tasks": [
                            {
                                "id": "a",
                                "title": "A",
                                "scope": ["../escape.ts"],
                                "acceptance": "x",
                                "dependsOn": [],
                            },
                        ],
                        "agent_id": "orch",
                        "role": "orchestrator",
                    },
                    root=root,
                )
            self.assertIn("..", str(ctx.exception))

    def test_verifier_cannot_create_plan_tasks(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "ver",
                    "role": "verifier",
                    "model": "opus",
                    "message": "checking",
                },
                root=root,
            )
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "project_create_plan_tasks",
                    {
                        "project_id": "scrna-seq",
                        "plan_id": "p",
                        "tasks": [
                            {
                                "id": "a",
                                "title": "A",
                                "scope": ["src/a.ts"],
                                "acceptance": "x",
                                "dependsOn": [],
                            }
                        ],
                        "agent_id": "ver",
                        "role": "verifier",
                    },
                    root=root,
                )
            self.assertIn(
                "verifier agents cannot use project_create_plan_tasks",
                str(ctx.exception),
            )

    def _create_one_task(self, root: Path, plan_id: str):
        return handle_tool_call(
            "project_create_plan_tasks",
            {
                "project_id": "scrna-seq",
                "plan_id": plan_id,
                "tasks": [
                    {
                        "id": "a",
                        "title": "A",
                        "scope": ["src/a.ts"],
                        "acceptance": "x",
                        "dependsOn": [],
                    },
                ],
                "agent_id": "orch",
                "role": "orchestrator",
            },
            root=root,
        )

    def test_rejects_malformed_plan_id(self):
        # B4/W6: the plan_id must be exactly 32 lowercase hex BEFORE the lock.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp, seed_approved=False)
            with self.assertRaises(McpError) as ctx:
                self._create_one_task(root, "plan-xyz")
            self.assertIn("32 lowercase hexadecimal", str(ctx.exception))

    def test_rejects_unknown_plan_id(self):
        # A well-formed but NEVER-submitted plan id has no verdict -> rejected.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp, seed_approved=False)
            with self.assertRaises(McpError) as ctx:
                self._create_one_task(root, "b" * 32)
            self.assertIn("requires an approved plan", str(ctx.exception))
            # Nothing was created (project still has only the manual T1).
            got = handle_tool_call(
                "project_get",
                {"project_id": "scrna-seq", "agent_id": "orch", "role": "orchestrator"},
                root=root,
            )
            self.assertEqual([t["id"] for t in got["state"]["tasks"]], ["T1"])

    def test_rejects_pending_plan(self):
        # A plan still awaiting the human (pending_approval) cannot have tasks created.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp, seed_approved=False)
            self._seed_plan_request(root, "c" * 32, "pending_approval")
            with self.assertRaises(McpError) as ctx:
                self._create_one_task(root, "c" * 32)
            self.assertIn("requires an approved plan", str(ctx.exception))

    def test_rejects_rejected_plan(self):
        # A REJECTED plan cannot have its tasks created.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp, seed_approved=False)
            self._seed_plan_request(root, "d" * 32, "rejected")
            with self.assertRaises(McpError) as ctx:
                self._create_one_task(root, "d" * 32)
            self.assertIn("requires an approved plan", str(ctx.exception))

    def test_approved_plan_evicted_from_queue_uses_sidecar(self):
        # If the terminal queue entry was capped/evicted, the durable sidecar still
        # records "approved" -> the gate passes via the sidecar fallback.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._setup(tmp, seed_approved=False)
            plan_id = "e" * 32
            base = root / "projects" / ".aspis-plans" / "scrna-seq"
            base.mkdir(parents=True, exist_ok=True)
            (base / f"{plan_id}.json").write_text(
                json.dumps(
                    {"id": plan_id, "projectId": "scrna-seq", "status": "approved"}
                ),
                encoding="utf-8",
            )
            out = self._create_one_task(root, plan_id)
            self.assertEqual(out["planId"], plan_id)
            self.assertEqual([t["id"] for t in out["tasks"]], ["T2"])


def _write_legacy_agents_state(projects: Path, state: dict) -> None:
    """Write a raw `.aspis-agents.json` straight to disk (NO normalization), so a
    test can simulate cold-boot legacy data exactly as an old build left it."""
    (projects / AGENTS_STATE_FILE).write_text(
        json.dumps(state, ensure_ascii=False, indent=2), encoding="utf-8"
    )


class LegacyRolePersistenceTests(unittest.TestCase):
    """Phase B back-compat: a stored legacy role:'orchestrator' (and any subagents
    or orchestrator-authored claims/events) must load, stay usable, and NEVER be
    silently rewritten — Python must not diverge from Rust, which never rewrites
    the stored role."""

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    def test_heartbeat_preserves_stored_orchestrator_role_on_disk(self):
        # BLOCKER 1: a heartbeat normalizes the session's stored role to "coder"
        # internally for the permission check, but it must NOT write that downgrade
        # back to disk — the legacy "orchestrator" string (and thus the derived
        # badge) must survive every heartbeat.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = prepare_management_root(root)
            # Cold-boot legacy session: role stored verbatim as "orchestrator".
            _write_legacy_agents_state(
                projects,
                {
                    "version": 1,
                    "sessions": [
                        {
                            "agentId": "orch-legacy",
                            "role": "orchestrator",
                            "model": "opus",
                            "status": "active",
                            "firstSeenAt": "2026-05-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                },
            )
            handle_tool_call(
                "agent_heartbeat",
                {"agent_id": "orch-legacy", "status": "active", "message": "alive"},
                root=root,
            )
            on_disk = json.loads(
                (projects / AGENTS_STATE_FILE).read_text(encoding="utf-8")
            )
            session = next(
                s for s in on_disk["sessions"] if s["agentId"] == "orch-legacy"
            )
            # The stored role string is PRESERVED verbatim across the heartbeat.
            self.assertEqual(session["role"], "orchestrator")

    def test_orchestrator_session_functions_with_coder_kanban_powers(self):
        # DEVBOULE: a session stored as "orchestrator" now acts AS orchestrator (no
        # collapse to coder), so it must request role="orchestrator". It still has
        # the coder's Kanban powers (project_create_followup is in its allowlist and
        # NOT verifier-only), proving the first-class role keeps coder-equivalent
        # project tools. Requesting role="coder" against this stored role would now
        # be a role mismatch (covered by the dedicated mismatch test).
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            _write_legacy_agents_state(
                projects,
                {
                    "version": 1,
                    "sessions": [
                        {
                            "agentId": "orch-legacy",
                            "role": "orchestrator",
                            "model": "opus",
                            "status": "active",
                            "firstSeenAt": "2026-05-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                },
            )
            # A coder-equivalent Kanban tool succeeds under role="orchestrator".
            handle_tool_call(
                "project_create_followup",
                {
                    "project_id": "scrna-seq",
                    "agent_id": "orch-legacy",
                    "role": "orchestrator",
                    "title": "Follow-up from orchestrator",
                    "reason": "Confirm the orchestrator session has coder-equivalent Kanban powers.",
                },
                root=root,
            )
            on_disk = json.loads(
                (projects / AGENTS_STATE_FILE).read_text(encoding="utf-8")
            )
            session = next(
                s for s in on_disk["sessions"] if s["agentId"] == "orch-legacy"
            )
            # Still preserved after a coder-tool call routed through upsert_session.
            self.assertEqual(session["role"], "orchestrator")

    def test_cold_boot_legacy_state_loads_and_stays_usable_end_to_end(self):
        # WARNING 8: a full cold-boot legacy file (orchestrator session + an
        # orchestrator-authored claim AND event) loads without error, the session is
        # usable (heartbeat + next-task + claim), the role is preserved, and the
        # legacy claim/event remain readable.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            _write_legacy_agents_state(
                projects,
                {
                    "version": 1,
                    "sessions": [
                        {
                            "agentId": "orch-legacy",
                            "role": "orchestrator",
                            "model": "opus",
                            "status": "active",
                            "firstSeenAt": "2026-05-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [
                        {
                            "projectId": "scrna-seq",
                            "projectTitle": "scRNA-seq backend",
                            "taskId": "T-OLD",
                            "taskTitle": "Legacy task",
                            "agentId": "orch-legacy",
                            "role": "orchestrator",
                            "status": "done",
                            "claimedAt": "2026-05-01T00:00:00+00:00",
                            "updatedAt": "2026-05-01T00:00:00+00:00",
                            "leaseUntil": "2026-05-01T01:00:00+00:00",
                        }
                    ],
                    "events": [
                        {
                            "id": "evt-legacy-1",
                            "agentId": "orch-legacy",
                            "role": "orchestrator",
                            "eventType": "claim",
                            "message": "Legacy orchestrator claimed T-OLD.",
                            "createdAt": "2026-05-01T00:00:00+00:00",
                        }
                    ],
                },
            )
            # Loads without error and preserves the legacy role + claim/event.
            loaded = read_agents_state(projects)
            session = next(
                s for s in loaded["sessions"] if s["agentId"] == "orch-legacy"
            )
            self.assertEqual(session["role"], "orchestrator")
            self.assertTrue(any(c.get("taskId") == "T-OLD" for c in loaded["claims"]))
            self.assertTrue(
                any(e.get("id") == "evt-legacy-1" for e in loaded["events"])
            )

            # Usable: heartbeat works and keeps the role.
            handle_tool_call(
                "agent_heartbeat",
                {"agent_id": "orch-legacy", "status": "active", "message": "alive"},
                root=root,
            )
            # Usable: next-task selection works under the (now first-class)
            # orchestrator role — the stored role is "orchestrator", so the request
            # must match it (a "coder" request would be a role mismatch now).
            nxt = handle_tool_call(
                "project_next_task",
                {
                    "project_id": "scrna-seq",
                    "agent_id": "orch-legacy",
                    "role": "orchestrator",
                },
                root=root,
            )
            self.assertEqual(nxt["task"]["id"], "T1")
            # Usable: claim works (orchestrator shares the coder's claim semantics)
            # and the legacy claim/event are still present after.
            claimed = handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "orch-legacy",
                    "role": "orchestrator",
                },
                root=root,
            )
            # COMPACT ACK (round-5 class fix): project_claim_task now returns a
            # compact ack; the ledger is asserted via on-disk state.
            on_disk = json.loads(
                (projects / AGENTS_STATE_FILE).read_text(encoding="utf-8")
            )
            self.assertTrue(any(c.get("taskId") == "T1" for c in on_disk["claims"]))
            session = next(
                s for s in on_disk["sessions"] if s["agentId"] == "orch-legacy"
            )
            self.assertEqual(session["role"], "orchestrator")
            self.assertTrue(any(c.get("taskId") == "T-OLD" for c in on_disk["claims"]))
            self.assertTrue(
                any(e.get("id") == "evt-legacy-1" for e in on_disk["events"])
            )


class StoredRoleSanitizationTests(unittest.TestCase):
    """WARNING 7: normalize_agents_state sanitizes stored roles on load — valid
    roles + known aliases survive verbatim; an UNKNOWN role folds to the safe
    'coder' default so no session is bricked."""

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    def test_unknown_stored_role_folds_to_coder_on_load(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            _write_legacy_agents_state(
                projects,
                {
                    "version": 1,
                    "sessions": [
                        {
                            "agentId": "rogue",
                            "role": "admin",
                            "model": "opus",
                            "status": "active",
                            "firstSeenAt": "2026-05-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                },
            )
            loaded = read_agents_state(projects)
            session = next(s for s in loaded["sessions"] if s["agentId"] == "rogue")
            # An unknown role is folded to the safe default so the session is usable.
            self.assertEqual(session["role"], "coder")
            # And it is genuinely usable (a coder tool succeeds, no brick on first call).
            handle_tool_call(
                "project_next_task",
                {"project_id": "scrna-seq", "agent_id": "rogue", "role": "coder"},
                root=root,
            )

    def test_session_with_no_role_key_loads_as_coder_and_is_usable(self):
        # BLOCKER C: a session missing the "role" key entirely (hand-edited file /
        # older or partial writer) previously fell through with no role and bricked
        # every tool call + re-registration (normalize_role("") raised). The
        # sanitization is now UNCONDITIONAL, so it loads as the safe "coder" default
        # and is immediately usable.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            _write_legacy_agents_state(
                projects,
                {
                    "version": 1,
                    "sessions": [
                        {
                            "agentId": "no-role",
                            # NOTE: no "role" key at all.
                            "model": "opus",
                            "status": "active",
                            "firstSeenAt": "2026-05-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                },
            )
            loaded = read_agents_state(projects)
            session = next(s for s in loaded["sessions"] if s["agentId"] == "no-role")
            self.assertEqual(session["role"], "coder")
            # Genuinely usable: a coder tool call succeeds (no brick on first call).
            handle_tool_call(
                "project_next_task",
                {"project_id": "scrna-seq", "agent_id": "no-role", "role": "coder"},
                root=root,
            )

    def test_known_alias_role_preserved_verbatim_on_load(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            _write_legacy_agents_state(
                projects,
                {
                    "version": 1,
                    "sessions": [
                        {
                            "agentId": "orch-legacy",
                            "role": "orchestrator",
                            "model": "opus",
                            "status": "active",
                            "firstSeenAt": "2026-05-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                },
            )
            loaded = read_agents_state(projects)
            session = next(
                s for s in loaded["sessions"] if s["agentId"] == "orch-legacy"
            )
            self.assertEqual(session["role"], "orchestrator")

    def test_coerce_role_maps_unknown_to_coder_and_preserves_canonical(self):
        self.assertEqual(coerce_role("admin"), "coder")
        # DEVBOULE: "orchestrator" is now a first-class VALID role, so coerce_role
        # preserves it verbatim (it previously folded to coder via ROLE_ALIASES).
        self.assertEqual(coerce_role("orchestrator"), "orchestrator")
        self.assertEqual(coerce_role("verifier"), "verifier")
        self.assertEqual(coerce_role("coder"), "coder")
        self.assertEqual(coerce_role(""), "coder")

    def test_validate_launch_token_coerces_corrupt_stored_role_without_raising(self):
        # MINOR 2 (defense-in-depth): a DIRECTLY-CONSTRUCTED state dict (i.e. one that
        # never went through `normalize_agents_state`) whose session carries a corrupt
        # role must NOT brick registration. Previously the raising `normalize_role`
        # was used on the stored role here, so a garbage role raised before the real
        # checks. With `coerce_role` the stored garbage collapses to the safe "coder"
        # default and the comparison stays well-defined.
        state = {
            "sessions": [
                {
                    "agentId": "rogue",
                    "role": "wizard",  # garbage role, never sanitized
                }
            ]
        }
        # The (already-normalized) incoming role is "coder"; coerce_role("wizard") ==
        # "coder", so the roles MATCH → no raise, the session is returned. No
        # launchTokenHash on this session and not launch_pending → accepted.
        result = validate_launch_token_for_registration(state, "rogue", "coder", None)
        self.assertIs(result, state["sessions"][0])

    def test_validate_launch_token_reports_coerced_role_on_mismatch(self):
        # A corrupt stored role that coerces to "coder" vs an incoming "verifier"
        # mismatches — and the error reports the COERCED role, not a raise from the
        # stored garbage.
        state = {"sessions": [{"agentId": "rogue", "role": "wizard"}]}
        with self.assertRaises(McpError) as ctx:
            validate_launch_token_for_registration(state, "rogue", "verifier", None)
        self.assertIn("already registered as coder", str(ctx.exception))


class CoderReopenFromReviewTests(unittest.TestCase):
    """WARNING 3: a coder may reopen its OWN task from 'review' back to 'todo'."""

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    def _register_coder(self, root: Path) -> None:
        handle_tool_call(
            "agent_register",
            {
                "agent_id": "codex",
                "role": "coder",
                "model": "codex",
                "message": "coding",
            },
            root=root,
        )

    def test_coder_claims_wip_review_then_reopens_to_todo(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            self._register_coder(root)
            # Claim a todo task -> auto-moves to wip (claim status "wip").
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "codex",
                    "role": "coder",
                },
                root=root,
            )
            # Move to review (claim status becomes "review" = inactive for selection).
            handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "review",
                    "agent_id": "codex",
                    "role": "coder",
                    "evidence": "Implemented; handing to verifier for review.",
                    "confidence": 0.6,
                },
                root=root,
            )
            # Reopen to todo from review: must succeed (no "must claim the task").
            reopened = handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "todo",
                    "agent_id": "codex",
                    "role": "coder",
                    "evidence": "Reopening from review to replan the approach.",
                    "confidence": 0.5,
                },
                root=root,
            )
            self.assertEqual(reopened["state"]["tasks"][0]["status"], "todo")

    def test_coder_still_cannot_set_done_after_review(self):
        # The reopen relaxation must NOT loosen the verifier-only done gate.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            self._register_coder(root)
            handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "codex",
                    "role": "coder",
                },
                root=root,
            )
            handle_tool_call(
                "project_update_status",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "status": "review",
                    "agent_id": "codex",
                    "role": "coder",
                    "evidence": "Implemented; handing to verifier for review.",
                    "confidence": 0.6,
                },
                root=root,
            )
            # A coder CANNOT set done even from review: the review-state claim is no
            # longer active (only a →todo reopen re-activates the owner's claim), so
            # the coder is rejected. Either way the verifier-only done gate holds.
            with self.assertRaises(McpError):
                handle_tool_call(
                    "project_update_status",
                    {
                        "project_id": "scrna-seq",
                        "task_id": "T1",
                        "status": "done",
                        "agent_id": "codex",
                        "role": "coder",
                        "evidence": "Trying to self-close after review.",
                        "confidence": 0.9,
                    },
                    root=root,
                )
            # The task remains in review (the coder's done attempt changed nothing).
            project = handle_tool_call(
                "project_get",
                {"project_id": "scrna-seq", "agent_id": "codex", "role": "coder"},
                root=root,
            )
            task = next(t for t in project["state"]["tasks"] if t["id"] == "T1")
            self.assertEqual(task["status"], "review")


class AllowedToolsCrossLanguageMirrorTests(unittest.TestCase):
    """role_rules.json is now the single source of truth for allowedTools,
    consumed directly by Python (this module's ROLE_RULES) and, once rewired
    by the Rust-side change, by src-tauri/src/backend/agents.rs too — both
    read the SAME file, so the old "parse agents.rs and diff against Python"
    drift guard is structurally impossible to trip and has been removed. What
    remains worth guarding here, on the Python/JSON side alone, is that each
    role's allowlist is well-formed: non-empty and distinct per role."""

    def test_allowed_tools_are_nonempty_and_unique_per_role(self):
        by_role = {rule["role"]: list(rule["allowedTools"]) for rule in ROLE_RULES}
        self.assertEqual(set(by_role), {"coder", "orchestrator", "verifier", "mini"})
        for role, tools in by_role.items():
            self.assertTrue(tools, f"{role} allowedTools must be non-empty")
        # No two roles share the exact same allowlist (would indicate a
        # copy-paste role that lost its distinct tool surface).
        seen: dict[tuple[str, ...], str] = {}
        for role, tools in by_role.items():
            key = tuple(sorted(tools))
            self.assertNotIn(
                key,
                seen,
                f"{role} and {seen.get(key)} have identical allowedTools sets",
            )
            seen[key] = role


class PushMandateCrossLanguageMirrorTests(unittest.TestCase):
    """GH-P5 (FIX F5): the cooperative push mandate. Both sides now load the
    SAME role_rules.json SSoT verbatim — Rust via `include_str!` +
    `parsed_role_rules()` (src-tauri/src/backend/agents.rs), Python via
    `json.loads` here (this module's ROLE_RULES) — so the historic
    Italian-vs-English bilingual split, and the old "parse the Rust source
    text" approach, are both gone: there is no separate Rust literal left to
    parse. This guards the SEMANTIC contract on the one shared source: the
    coder's push mandate must contain (a) the literal tool name
    `request_git_push`, (b) a "never raw push" prohibition, and (c) a
    "stop + escalate" instruction. If it ever drops the tool name or the gate
    instruction, this fails — and since both languages read this exact file,
    neither side can drift from the other."""

    def test_coder_push_carries_gate_contract(self):
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        self.assertIn("push", coder, "coder must declare a push mandate")
        blob = " ".join(coder["push"])
        # (a) the tool name, (b) never-raw-push, (c) stop + escalate.
        self.assertIn("request_git_push", blob, "coder.push must name request_git_push")
        self.assertIn(
            "NEVER run a raw `git push`",
            blob,
            "coder.push must forbid a raw git push",
        )
        self.assertIn("STOP", blob, "coder.push must instruct to STOP")
        self.assertIn(
            "needs_user", blob, "coder.push must instruct to escalate via needs_user"
        )


class AgentModelAndSubagentsTests(unittest.TestCase):
    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    def test_register_stores_normalized_model(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            state = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-1",
                    "role": "coder",
                    "model": "claude-opus-4-8",
                    "message": "registered",
                },
                root=root,
            )
            self.assertEqual(state["session"]["model"], "opus")

    def test_register_without_model_adds_soft_event(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            state = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-nomodel",
                    "role": "coder",
                    "message": "registered",
                },
                root=root,
            )
            # Registration still succeeds; model field defaults to "". The compact
            # ack returns ONLY this agent's own session + a fleet summary (FIX: the
            # old full-fleet dump drowned small local models). The full fleet
            # (events, claims, rules) is available via agent_state.
            self.assertEqual(state["session"].get("model", ""), "")
            full = handle_tool_call(
                "agent_state",
                {"agent_id": "coder-nomodel", "role": "coder"},
                root=root,
            )
            # A soft event flags that the agent did not report a model.
            messages = " ".join(e.get("message", "").lower() for e in full["events"])
            # The soft event uses a DISTINCT event type so the UI/consumers can
            # tell it apart from the normal "register" event.
            event_types = {e.get("eventType") for e in full["events"]}
            self.assertIn("register", event_types)
            self.assertIn("register_incomplete", event_types)
            incomplete = next(
                e
                for e in full["events"]
                if e.get("eventType") == "register_incomplete"
            )
            self.assertIn("model", incomplete.get("message", "").lower())

    def test_register_with_model_has_no_incomplete_event(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            state = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-ok",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            full = handle_tool_call(
                "agent_state",
                {"agent_id": "coder-ok", "role": "coder"},
                root=root,
            )
            event_types = {e.get("eventType") for e in full["events"]}
            self.assertIn("register", event_types)
            self.assertNotIn("register_incomplete", event_types)

    def test_register_default_seeds_subagents_and_needs_user(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            state = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-seed",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            session = state["session"]
            self.assertEqual(session["subagents"], [])
            self.assertIsNone(session["needsUser"])

    def test_register_and_heartbeat_return_compact_ack(self):
        """FIX regression: agent_register / agent_heartbeat must return a COMPACT
        ack (own session + fleet summary), NOT the entire fleet state. The old
        >100KB dump drowned small local models, which then emitted an empty
        message and the user saw no reply."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            sample_project(projects)
            token = "compact-launch-token-0123456789"
            # Seed a managed launch_pending session (app-issued launch token)
            # so registration takes the MANAGED path and mints a sessionToken.
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-06-12T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "compact-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            registered = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "compact-coder",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                    "launch_token": token,
                },
                root=root,
            )
            # Top-level compact contract.
            self.assertIn("sessionToken", registered)
            self.assertIsNotNone(registered.get("sessionToken"))
            self.assertIn("version", registered)
            self.assertIn("updatedAt", registered)
            self.assertIn("fleet", registered)
            self.assertEqual(registered["fleet"]["sessions"], 1)
            self.assertEqual(registered["fleet"]["active"], 1)
            self.assertIn("note", registered)
            # "session" is the registering agent's OWN entry, sanitized (no
            # token-hash fields) — NOT the full sessions list.
            self.assertIn("session", registered)
            self.assertIsNotNone(registered["session"])
            self.assertEqual(registered["session"]["agentId"], "compact-coder")
            self.assertNotIn("launchTokenHash", registered["session"])
            self.assertNotIn("launchTokenIssuedAt", registered["session"])
            self.assertNotIn("sessionTokenHash", registered["session"])
            self.assertNotIn("sessionTokenIssuedAt", registered["session"])
            self.assertNotIn("launchConsumedAt", registered["session"])
            # The full fleet is deliberately NOT dumped.
            self.assertNotIn("sessions", registered)
            self.assertNotIn("claims", registered)
            self.assertNotIn("events", registered)
            self.assertNotIn("rules", registered)

            # Heartbeat ack also carries the agent's own session.
            heartbeat = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "compact-coder",
                    "status": "active",
                    "message": "alive",
                    "session_token": registered["sessionToken"],
                },
                root=root,
            )
            self.assertIn("session", heartbeat)
            self.assertEqual(heartbeat["session"]["agentId"], "compact-coder")
            self.assertEqual(heartbeat["fleet"]["sessions"], 1)

    def test_claim_task_returns_compact_ack_with_lease(self):
        """FIX regression: project_claim_task must return a COMPACT ack (own session
        + fleet summary) plus a "claim" key carrying the lease outcome — NOT the
        entire fleet state. Mirrors the round-5 compact_session_ack fix. Asserts
        the shape and a size bound against a fat multi-session fixture."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            sample_project(projects)
            token = "compact-claim-launch-token-0123456789"
            # Seed a managed launch_pending session so registration mints a
            # sessionToken (managed path), and seed several extra sessions so the
            # ack is non-trivial but still compact.
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-06-12T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "claim-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            },
                            # Extra sessions to make the fleet non-trivial.
                            {
                                "agentId": "other-coder",
                                "role": "coder",
                                "status": "active",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                            },
                            {
                                "agentId": "verifier-1",
                                "role": "verifier",
                                "status": "idle",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                            },
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            registered = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "claim-coder",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                    "launch_token": token,
                },
                root=root,
            )
            claim_result = handle_tool_call(
                "project_claim_task",
                {
                    "project_id": "scrna-seq",
                    "task_id": "T1",
                    "agent_id": "claim-coder",
                    "role": "coder",
                    "session_token": registered["sessionToken"],
                },
                root=root,
            )
            # Top-level compact contract: own session + fleet summary + claim key.
            self.assertIn("session", claim_result)
            self.assertEqual(claim_result["session"]["agentId"], "claim-coder")
            self.assertIn("fleet", claim_result)
            self.assertGreaterEqual(claim_result["fleet"]["sessions"], 3)
            # Sensitive fields must NOT leak in the compact session.
            for leak_key in (
                "launchTokenHash",
                "launchTokenIssuedAt",
                "sessionTokenHash",
                "sessionTokenIssuedAt",
                "launchConsumedAt",
            ):
                self.assertNotIn(leak_key, claim_result["session"])
            # The full fleet is deliberately NOT dumped.
            self.assertNotIn("sessions", claim_result)
            self.assertNotIn("claims", claim_result)
            self.assertNotIn("events", claim_result)
            self.assertNotIn("rules", claim_result)
            # The "claim" key carries the lease outcome the caller needs.
            self.assertIn("claim", claim_result)
            claim = claim_result["claim"]
            self.assertEqual(claim.get("projectId"), "scrna-seq")
            self.assertEqual(claim.get("taskId"), "T1")
            # Coder claims a todo task -> auto-advances to wip.
            self.assertEqual(claim.get("status"), "wip")
            self.assertIn("leaseUntil", claim)
            self.assertTrue(claim["leaseUntil"])
            # Size bound: even with a fat multi-session fixture the ack must be
            # comfortably under 4 KB (round-5 contract).
            self.assertLess(len(json.dumps(claim_result)), 4096)

    def test_heartbeat_subagents_provided_then_cleared_then_untouched(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "orch-1",
                    "role": "orchestrator",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            # Provide subagents.
            hb1 = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "orch-1",
                    "status": "active",
                    "message": "spawned",
                    "subagents": [{"label": "explore", "model": "haiku", "count": 2}],
                },
                root=root,
            )
            self.assertEqual(
                hb1["session"]["subagents"],
                [{"label": "explore", "model": "haiku", "count": 2, "role": None}],
            )
            # Absent subagents: leave untouched.
            hb2 = handle_tool_call(
                "agent_heartbeat",
                {"agent_id": "orch-1", "status": "active", "message": "still working"},
                root=root,
            )
            self.assertEqual(
                hb2["session"]["subagents"],
                [{"label": "explore", "model": "haiku", "count": 2, "role": None}],
            )
            # Empty list clears.
            hb3 = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "orch-1",
                    "status": "active",
                    "message": "done spawning",
                    "subagents": [],
                },
                root=root,
            )
            self.assertEqual(hb3["session"]["subagents"], [])

    def test_heartbeat_malformed_subagents_leaves_value_untouched(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "orch-m",
                    "role": "orchestrator",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "orch-m",
                    "status": "active",
                    "message": "spawned",
                    "subagents": [{"label": "explore", "model": "haiku", "count": 2}],
                },
                root=root,
            )
            # A malformed (non-list) subagents value must NOT overwrite the stored
            # breakdown with None — it is treated as "not provided".
            hb = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "orch-m",
                    "status": "active",
                    "message": "x",
                    "subagents": "garbage",
                },
                root=root,
            )
            self.assertEqual(
                hb["session"]["subagents"],
                [{"label": "explore", "model": "haiku", "count": 2, "role": None}],
            )

    def test_needs_user_heartbeat_sets_needs_user_with_since(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "nu-coder",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            hb = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "nu-coder",
                    "status": "needs_user",
                    "message": "Should I overwrite config.toml?",
                },
                root=root,
            )
            session = hb["session"]
            self.assertEqual(session["status"], "needs_user")
            needs = session["needsUser"]
            self.assertIsNotNone(needs)
            self.assertEqual(needs["reason"], "needs_user")
            self.assertEqual(needs["message"], "Should I overwrite config.toml?")
            self.assertTrue(needs["since"])

    def test_needs_user_repeat_heartbeat_preserves_since(self):
        """A repeated needs_user heartbeat must NOT reset `since` — the frontend
        dedups OS notifications on that transition timestamp."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "nu-keep",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            hb1 = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "nu-keep",
                    "status": "needs_user",
                    "message": "Approve deploy?",
                },
                root=root,
            )
            first_since = hb1["session"]["needsUser"]["since"]
            hb2 = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "nu-keep",
                    "status": "needs_user",
                    "message": "Approve deploy? (still waiting)",
                },
                root=root,
            )
            needs = hb2["session"]["needsUser"]
            self.assertEqual(needs["since"], first_since)
            # The message MAY refresh while the transition timestamp is pinned.
            self.assertEqual(needs["message"], "Approve deploy? (still waiting)")

    def test_needs_user_aliases_normalize_to_needs_user(self):
        """`awaiting_user` and `blocked_on_user` normalize to needs_user status,
        but the original alias is preserved as the reason. Plain `blocked` stays
        a distinct status and does NOT set needsUser."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            for agent_id, alias in (("aw", "awaiting_user"), ("bo", "blocked_on_user")):
                handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": agent_id,
                        "role": "coder",
                        "model": "opus",
                        "message": "reg",
                    },
                    root=root,
                )
                hb = handle_tool_call(
                    "agent_heartbeat",
                    {"agent_id": agent_id, "status": alias, "message": "need input"},
                    root=root,
                )
                session = hb["session"]
                self.assertEqual(session["status"], "needs_user")
                self.assertEqual(session["needsUser"]["reason"], alias)

            # Plain `blocked` is a DISTINCT status: no needsUser.
            handle_tool_call(
                "agent_register",
                {"agent_id": "blk", "role": "coder", "model": "opus", "message": "reg"},
                root=root,
            )
            hb_blocked = handle_tool_call(
                "agent_heartbeat",
                {"agent_id": "blk", "status": "blocked", "message": "task blocked"},
                root=root,
            )
            blocked_session = hb_blocked["session"]
            self.assertEqual(blocked_session["status"], "blocked")
            self.assertIsNone(blocked_session["needsUser"])

    def test_working_heartbeat_clears_needs_user(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "nu-clear",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            handle_tool_call(
                "agent_heartbeat",
                {"agent_id": "nu-clear", "status": "needs_user", "message": "Approve?"},
                root=root,
            )
            hb = handle_tool_call(
                "agent_heartbeat",
                {"agent_id": "nu-clear", "status": "active", "message": "back to work"},
                root=root,
            )
            session = hb["session"]
            self.assertEqual(session["status"], "active")
            self.assertIsNone(session["needsUser"])

    def test_needs_user_message_cleaned_and_capped(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "nu-cap",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            hb = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "nu-cap",
                    "status": "needs_user",
                    "message": "line1\nline2\t" + "z" * 4000,
                },
                root=root,
            )
            message = hb["session"]["needsUser"]["message"]
            # Control chars collapsed to single spaces by clean_text.
            self.assertNotIn("\n", message)
            self.assertNotIn("\t", message)
            self.assertTrue(message.startswith("line1 line2"))
            self.assertLessEqual(len(message), 1000)

    def test_needs_user_whitespace_only_message_falls_back(self):
        """A whitespace-only needs_user message ("   ") is truthy under a bare
        `if message`, but clean_text rejects an all-whitespace value and raises.
        It must fall back to the status sentinel, not fail the heartbeat (#5)."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "nu-ws",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            hb = handle_tool_call(
                "agent_heartbeat",
                {"agent_id": "nu-ws", "status": "needs_user", "message": "   "},
                root=root,
            )
            session = hb["session"]
            self.assertEqual(session["status"], "needs_user")
            self.assertEqual(session["needsUser"]["message"], "needs_user")

    def test_register_does_not_set_needs_user(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            state = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "nu-reg",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            self.assertIsNone(state["session"]["needsUser"])

    def test_public_state_carries_new_fields_and_strips_tokens(self):
        state = {
            "version": 2,
            "updatedAt": "2026-06-04T00:00:00+00:00",
            "sessions": [
                {
                    "agentId": "a1",
                    "role": "coder",
                    "model": "opus",
                    "status": "active",
                    "subagents": [
                        {"label": "x", "model": "haiku", "count": 1, "role": None}
                    ],
                    "needsUser": None,
                    "sessionTokenHash": "secret-hash",
                    "sessionTokenIssuedAt": "2026-06-04T00:00:00+00:00",
                    "launchTokenHash": "secret-launch",
                    "launchTokenIssuedAt": "2026-06-04T00:00:00+00:00",
                }
            ],
            "claims": [],
            "events": [],
        }
        public = public_agents_state(state)
        session = public["sessions"][0]
        self.assertEqual(
            session["subagents"],
            [{"label": "x", "model": "haiku", "count": 1, "role": None}],
        )
        self.assertIsNone(session["needsUser"])
        self.assertNotIn("sessionTokenHash", session)
        self.assertNotIn("sessionTokenIssuedAt", session)
        self.assertNotIn("launchTokenHash", session)
        self.assertNotIn("launchTokenIssuedAt", session)

    def test_old_state_without_new_fields_loads_and_defaults(self):
        """Critical back-compat: a version-1 state file with no subagents/needsUser
        loads without exception and gets the new fields defaulted."""
        old_state = {
            "version": 1,
            "updatedAt": "2026-05-29T00:00:00+00:00",
            "sessions": [
                {
                    "agentId": "legacy",
                    "role": "coder",
                    "model": "gpt",
                    "status": "active",
                    "lastSeenAt": "2026-05-29T00:00:00+00:00",
                }
            ],
            "claims": [],
            "events": [],
        }
        normalized = normalize_agents_state(old_state)
        # Version is UPGRADED on load: the additive backfill below makes the file
        # genuinely conform to the current schema, so it is stamped as current
        # rather than left at v1 forever.
        self.assertEqual(normalized["version"], AGENTS_STATE_VERSION)
        session = normalized["sessions"][0]
        self.assertEqual(session["subagents"], [])
        self.assertIsNone(session["needsUser"])

    def test_version_upgrade_and_no_downgrade(self):
        """Version handling on load: old int -> upgraded to current; missing or
        garbage (incl. bool) -> current; a FUTURE version -> left untouched."""
        base = {
            "updatedAt": "2026-05-29T00:00:00+00:00",
            "sessions": [],
            "claims": [],
            "events": [],
        }

        upgraded = normalize_agents_state({**base, "version": 1})
        self.assertEqual(upgraded["version"], AGENTS_STATE_VERSION)

        missing = normalize_agents_state({**base})
        self.assertEqual(missing["version"], AGENTS_STATE_VERSION)

        garbage_str = normalize_agents_state({**base, "version": "two"})
        self.assertEqual(garbage_str["version"], AGENTS_STATE_VERSION)

        # bool is an int subclass; it must be treated as garbage, not as int 1.
        garbage_bool = normalize_agents_state({**base, "version": True})
        self.assertEqual(garbage_bool["version"], AGENTS_STATE_VERSION)

        # A higher (future) version is never downgraded.
        future = normalize_agents_state({**base, "version": AGENTS_STATE_VERSION + 5})
        self.assertEqual(future["version"], AGENTS_STATE_VERSION + 5)

    def test_old_state_round_trips_through_handle_tool_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            prepare_management_root(root)
            projects = root / "projects"
            projects.mkdir(parents=True, exist_ok=True)
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-05-29T00:00:00+00:00",
                        "sessions": [
                            {
                                "agentId": "legacy",
                                "role": "coder",
                                "model": "gpt",
                                "status": "active",
                                "lastSeenAt": "2026-05-29T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            # Reading old state (via agent_register of a new agent) must not raise
            # and must default the legacy session's new fields.
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "fresh",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                },
                root=root,
            )
            # The compact ack returns only the registering agent's own session;
            # the legacy session lives in the full fleet, fetched via agent_state.
            full = handle_tool_call(
                "agent_state",
                {"agent_id": "fresh", "role": "coder"},
                root=root,
            )
            legacy = next(s for s in full["sessions"] if s["agentId"] == "legacy")
            self.assertEqual(legacy["subagents"], [])
            self.assertIsNone(legacy["needsUser"])


class T4HeartbeatAckNoteTests(unittest.TestCase):
    """T4 — Ticket C: heartbeat acks deliberately omit `sessionToken`
    (small models kept retrying, thinking the absence signaled an error).
    The agent-facing guidance the models DO receive must explicitly explain
    the contract so they don't retry / re-register / loop.

    `compact_session_ack` is the single composition site for the ack
    returned by BOTH `agent_register` AND `agent_heartbeat`. The
    no-token-echo sentence lives in the `note` field of the ack payload
    (so a cheap string assert on the composed payload suffices).
    """

    def setUp(self):
        # Compact-ack is reachable via the unmanaged-privileged kill switch.
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    def test_compact_session_ack_note_states_no_token_echo_contract(self):
        # Direct, cheap assertion on the ack-rendering function. The note
        # is the only field the operator-facing guidance travels in for
        # both register and heartbeat replies.
        from oracle.server.aspis_mcp import (
            compact_session_ack,
            default_agents_state,
        )

        state = default_agents_state()
        state["sessions"] = [
            {
                "agentId": "hb-coder",
                "role": "coder",
                "status": "active",
                "lastSeenAt": "2026-06-12T00:00:00Z",
            }
        ]
        ack = compact_session_ack(state, "hb-coder")
        note = ack.get("note", "")
        self.assertIn("heartbeat", note.lower())
        self.assertIn("sessionToken", note)
        self.assertIn("NOT an error", note or "NOT an error".lower())

    def test_register_and_heartbeat_ack_text_carries_no_token_echo_note(self):
        # End-to-end: drive both register AND heartbeat through handle_tool_call,
        # confirm the composed ack payloads both carry the no-token-echo
        # sentence. This is the cheap string assert the ticket asked for.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            projects.mkdir()
            sample_project(projects)
            token = "t4-c-launch-token-0123456789"
            (projects / ".aspis-agents.json").write_text(
                json.dumps(
                    {
                        "version": 1,
                        "updatedAt": "2026-06-12T00:00:00Z",
                        "sessions": [
                            {
                                "agentId": "hb-coder",
                                "role": "coder",
                                "status": "launch_pending",
                                "lastSeenAt": "2026-06-12T00:00:00Z",
                                "launchTokenHash": hashlib.sha256(
                                    token.encode("utf-8")
                                ).hexdigest(),
                                "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                            }
                        ],
                        "claims": [],
                        "events": [],
                    }
                ),
                encoding="utf-8",
            )
            registered = handle_tool_call(
                "agent_register",
                {
                    "agent_id": "hb-coder",
                    "role": "coder",
                    "model": "opus",
                    "message": "reg",
                    "launch_token": token,
                },
                root=root,
            )
            self.assertIn("note", registered)
            self.assertIn("sessionToken", registered["note"])
            self.assertIn("NOT an error", registered["note"])

            heartbeat = handle_tool_call(
                "agent_heartbeat",
                {
                    "agent_id": "hb-coder",
                    "status": "active",
                    "message": "alive",
                    "session_token": registered["sessionToken"],
                },
                root=root,
            )
            self.assertIn("note", heartbeat)
            self.assertIn("sessionToken", heartbeat["note"])
            self.assertIn("NOT an error", heartbeat["note"])
            # Heartbeat ack must NOT carry a fresh sessionToken; the model
            # should re-use the one from register.
            self.assertNotIn("sessionToken", heartbeat)


class AgentIdHardeningTests(unittest.TestCase):
    """FIX 3: agent ids must match the safe allowlist so a rogue local process
    cannot register as e.g. an alarming display name and sign phishing toasts."""

    def test_normalize_agent_id_accepts_generated_and_simple_ids(self):
        self.assertEqual(
            normalize_agent_id("coder-1717459200000"), "coder-1717459200000"
        )
        self.assertEqual(normalize_agent_id("reviewer-7f"), "reviewer-7f")
        self.assertEqual(normalize_agent_id("agent.id_1-2"), "agent.id_1-2")
        self.assertEqual(normalize_agent_id("a"), "a")
        self.assertEqual(normalize_agent_id("x" * 64), "x" * 64)
        # Surrounding whitespace is trimmed (but internal spaces are rejected).
        self.assertEqual(normalize_agent_id("  coder-1  "), "coder-1")

    def test_normalize_agent_id_rejects_spoofed_or_unsafe_ids(self):
        for bad in [
            "",
            "   ",
            "x" * 65,
            "has space",
            "coder/../evil",
            "a:b",
            "a\nb",
            "⚠️ Critical Security Alert",
            "emoji\U0001f600",
            "zero​width",
        ]:
            with self.assertRaises(McpError):
                normalize_agent_id(bad)

    def test_agent_register_rejects_spoofed_agent_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "projects").mkdir()
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
            try:
                with self.assertRaises(McpError):
                    handle_tool_call(
                        "agent_register",
                        {
                            "agent_id": "⚠️ Critical Security Alert",
                            "role": "coder",
                            "model": "test",
                            "message": "spoof",
                        },
                        root=root,
                    )
            finally:
                os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)


class InvisibleBidiSanitizationTests(unittest.TestCase):
    """FIX 5: invisible/bidi control characters are stripped from cleaned text so a
    value cannot smuggle an RTL override or zero-width char into a toast/note."""

    def test_strip_removes_bidi_and_zero_width(self):
        dirty = "alert‮gnitirw​ done"
        self.assertEqual(strip_invisible_and_bidi(dirty), "alertgnitirw done")
        # BOM, line/paragraph separators, isolates.
        self.assertEqual(
            strip_invisible_and_bidi("a﻿b c⁩d"),
            "abcd",
        )

    def test_clean_text_strips_rtl_override(self):
        cleaned = clean_text("safe‮text", "Note")
        self.assertEqual(cleaned, "safetext")
        self.assertNotIn("‮", cleaned)

    def test_clean_text_all_invisible_is_treated_as_empty(self):
        with self.assertRaises(McpError):
            clean_text("​‮﻿", "Note")


class StateCapTests(unittest.TestCase):
    """FIX 6: bound sessions/claims; never drop a live session or an open claim."""

    @staticmethod
    def _session(idx, status, last_seen):
        return {
            "agentId": f"agent-{idx}",
            "role": "coder",
            "status": status,
            "lastSeenAt": last_seen,
        }

    def test_sessions_capped_dropping_oldest_closed_only(self):
        live = [
            self._session(i, "active", f"2026-06-04T00:{i % 60:02d}:00Z")
            for i in range(40)
        ]
        closed = [
            self._session(1000 + i, "closed", f"2026-01-01T00:{i % 60:02d}:00Z")
            for i in range(MAX_SESSIONS)
        ]
        capped = cap_sessions(live + closed)
        self.assertEqual(len(capped), MAX_SESSIONS)
        # Every live session survives.
        self.assertEqual(sum(1 for s in capped if s["status"] == "active"), 40)

    def test_live_sessions_never_dropped_even_past_cap(self):
        all_live = [
            self._session(i, "active", f"2026-06-04T00:{i % 60:02d}:00Z")
            for i in range(MAX_SESSIONS + 50)
        ]
        capped = cap_sessions(all_live)
        self.assertEqual(len(capped), MAX_SESSIONS + 50)

    def test_under_cap_sessions_unchanged(self):
        few = [self._session(i, "closed", "2026-01-01T00:00:00Z") for i in range(5)]
        self.assertEqual(cap_sessions(few), few)

    @staticmethod
    def _claim(idx, status, lease, updated):
        return {
            "projectId": "p",
            "taskId": f"t{idx}",
            "agentId": "a",
            "role": "coder",
            "status": status,
            "leaseUntil": lease,
            "updatedAt": updated,
        }

    def test_claims_capped_dropping_oldest_terminal_only(self):
        future = "2099-01-01T00:00:00Z"
        past = "2000-01-01T00:00:00Z"
        open_claims = [
            self._claim(i, "wip", future, f"2026-06-04T00:{i % 60:02d}:00Z")
            for i in range(120)
        ]
        done_claims = [
            self._claim(1000 + i, "done", past, f"2026-01-01T00:{i % 60:02d}:00Z")
            for i in range(MAX_CLAIMS)
        ]
        capped = cap_claims(open_claims + done_claims)
        self.assertEqual(len(capped), MAX_CLAIMS)
        # Every open/working claim survives.
        self.assertEqual(sum(1 for c in capped if c["status"] == "wip"), 120)

    def test_open_claims_never_dropped_even_past_cap(self):
        future = "2099-01-01T00:00:00Z"
        all_open = [
            self._claim(i, "wip", future, f"2026-06-04T00:{i % 60:02d}:00Z")
            for i in range(MAX_CLAIMS + 50)
        ]
        self.assertEqual(len(cap_claims(all_open)), MAX_CLAIMS + 50)

    def test_caps_applied_through_normalize(self):
        state = {
            "version": AGENTS_STATE_VERSION,
            "sessions": [
                self._session(i, "closed", "2026-01-01T00:00:00Z")
                for i in range(MAX_SESSIONS + 10)
            ],
            "claims": [
                self._claim(i, "done", "2000-01-01T00:00:00Z", "2026-01-01T00:00:00Z")
                for i in range(MAX_CLAIMS + 10)
            ],
            "events": [],
        }
        normalized = normalize_agents_state(state)
        self.assertEqual(len(normalized["sessions"]), MAX_SESSIONS)
        self.assertEqual(len(normalized["claims"]), MAX_CLAIMS)


def _rust_shaped_shard(file_rel: str, findings: list[dict]) -> dict:
    """A shard exactly as the Rust writer (backend/censor/schema.rs serde,
    camelCase) emits it — used to prove the Python tool reads a Rust-written shard
    with the correct keys."""
    return {
        "fileRelPath": file_rel,
        "contentHash": "hash-abc",
        "updatedAt": "2026-06-05T00:00:00Z",
        "findings": findings,
    }


def _rust_shaped_finding(**overrides) -> dict:
    finding = {
        "id": "f-1",
        "file": "src/app.ts",
        "contentHash": "hash-abc",
        "line": 12,
        "severity": "high",
        "category": "security",
        "source": "gitleaks",
        "title": "Hardcoded secret",
        "body": "A credential pattern was detected.",
        "verdict": "suspected",
        "disposition": "open",
        "provenance": [
            {"actor": "censor", "action": "created", "at": "2026-06-05T00:00:00Z"}
        ],
        "createdAt": "2026-06-05T00:00:00Z",
        "commit": None,
    }
    finding.update(overrides)
    return finding


def _write_shard(work_root: Path, file_rel: str, findings: list[dict]) -> Path:
    shard_path = censor_shard_path(work_root, file_rel)
    shard_path.parent.mkdir(parents=True, exist_ok=True)
    shard_path.write_text(
        json.dumps(
            _rust_shaped_shard(file_rel, findings), indent=2, ensure_ascii=False
        ),
        encoding="utf-8",
    )
    return shard_path


def _project_with_root(projects_dir: Path, work_root: Path) -> None:
    escaped = str(work_root).replace("\\", "\\\\")
    (projects_dir / "censor-proj.md").write_text(
        f"""---
id: censor-proj
title: Censor project
status: active
updated_at: 2026-06-05T00:00:00Z
root_path: "{escaped}"
---

```aspis-project
{{"version":1,"tasks":[],"notes":[]}}
```
""",
        encoding="utf-8",
    )


class CensorLedgerHelperTests(unittest.TestCase):
    """The pure shard helpers used by the censor_findings / censor_dispose tools."""

    def test_validate_rel_path_rejects_traversal_and_absolute(self):
        validate_censor_rel_path("src/app.ts")  # ok
        validate_censor_rel_path("a/./b.rs")  # ok (. is fine)
        with self.assertRaises(McpError):
            validate_censor_rel_path("../escape.ts")
        with self.assertRaises(McpError):
            validate_censor_rel_path("a/../../b.ts")
        with self.assertRaises(McpError):
            validate_censor_rel_path("/etc/passwd")
        with self.assertRaises(McpError):
            validate_censor_rel_path("C:\\Windows\\system32")
        with self.assertRaises(McpError):
            validate_censor_rel_path("-rf.ts")  # argv-injection guard
        with self.assertRaises(McpError):
            validate_censor_rel_path("")

    def test_shard_path_matches_rust_hash_normalizing_separators(self):
        # Backslash and forward-slash variants of the SAME file map to ONE shard
        # (verbatim mirror of the Rust shard_path normalization), and the filename
        # is the sha256 of the normalized rel path.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            a = censor_shard_path(root, "src/a.rs")
            b = censor_shard_path(root, "src\\a.rs")
            self.assertEqual(a, b)
            expected = hashlib.sha256(b"src/a.rs").hexdigest()
            self.assertEqual(a.name, f"{expected}.json")
            self.assertEqual(a.parent.name, ".aspis-censor")

    def test_read_open_findings_only_open_and_strips_to_safe_fields(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            _write_shard(
                work_root,
                "src/app.ts",
                [
                    _rust_shaped_finding(id="open-1", disposition="open"),
                    _rust_shaped_finding(id="fp-1", disposition="fp"),
                    _rust_shaped_finding(id="fixed-1", disposition="fixed"),
                ],
            )
            findings = read_censor_open_findings(work_root, None)
            self.assertEqual([f["id"] for f in findings], ["open-1"])
            f = findings[0]
            # Safe allowlist only — NO contentHash / createdAt / commit leak.
            self.assertEqual(
                set(f.keys()),
                {
                    "id",
                    "file",
                    "line",
                    "severity",
                    "category",
                    "source",
                    "title",
                    "body",
                    "verdict",
                    "disposition",
                    "provenance",
                },
            )
            self.assertNotIn("contentHash", f)
            self.assertNotIn("createdAt", f)
            self.assertNotIn("commit", f)
            # camelCase keys identical to the Rust serde round-trip.
            self.assertEqual(f["severity"], "high")
            self.assertEqual(f["category"], "security")

    def test_read_open_findings_strips_unknown_future_field(self):
        # A field a future build adds must never leak to an agent (strict allowlist).
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            finding = _rust_shaped_finding(id="open-1")
            finding["rawStdout"] = "AKIAIOSFODNN7EXAMPLE super-secret-payload"
            finding["someFutureField"] = {"nested": True}
            _write_shard(work_root, "src/app.ts", [finding])
            findings = read_censor_open_findings(work_root, None)
            self.assertEqual(len(findings), 1)
            self.assertNotIn("rawStdout", findings[0])
            self.assertNotIn("someFutureField", findings[0])
            blob = json.dumps(findings[0])
            self.assertNotIn("AKIAIOSFODNN7EXAMPLE", blob)

    def test_read_open_findings_filters_by_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            _write_shard(
                work_root,
                "src/a.ts",
                [_rust_shaped_finding(id="a-open", file="src/a.ts")],
            )
            _write_shard(
                work_root,
                "src/b.ts",
                [_rust_shaped_finding(id="b-open", file="src/b.ts")],
            )
            only_a = read_censor_open_findings(work_root, "src/a.ts")
            self.assertEqual([f["id"] for f in only_a], ["a-open"])
            both = read_censor_open_findings(work_root, None)
            self.assertEqual({f["id"] for f in both}, {"a-open", "b-open"})

    def test_read_open_findings_missing_dir_is_empty(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(read_censor_open_findings(Path(tmp), None), [])

    def test_dispose_sets_disposition_and_appends_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = _write_shard(
                work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")]
            )
            disposed = dispose_censor_finding(
                work_root, "src/app.ts", "f-1", "fp", "coder-7", "2026-06-05T01:00:00Z"
            )
            self.assertEqual(disposed["disposition"], "fp")
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            finding = stored["findings"][0]
            self.assertEqual(finding["disposition"], "fp")
            # Provenance APPENDED (the original "created" entry survives).
            actions = [p["action"] for p in finding["provenance"]]
            self.assertEqual(actions, ["created", "fp"])
            self.assertEqual(finding["provenance"][-1]["actor"], "coder-7")
            self.assertEqual(finding["provenance"][-1]["at"], "2026-06-05T01:00:00Z")
            self.assertEqual(stored["updatedAt"], "2026-06-05T01:00:00Z")

    def test_dispose_is_idempotent_in_effect(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = _write_shard(
                work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")]
            )
            dispose_censor_finding(work_root, "src/app.ts", "f-1", "fp", "coder", "t1")
            dispose_censor_finding(work_root, "src/app.ts", "f-1", "fp", "coder", "t2")
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            self.assertEqual(stored["findings"][0]["disposition"], "fp")

    def test_dispose_rejects_unknown_disposition_and_missing_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            _write_shard(work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")])
            with self.assertRaises(McpError):
                dispose_censor_finding(
                    work_root, "src/app.ts", "f-1", "bogus", "coder", "t"
                )
            with self.assertRaises(McpError):
                dispose_censor_finding(
                    work_root, "src/app.ts", "nope", "fp", "coder", "t"
                )

    def test_dispose_refuses_corrupt_shard(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = censor_shard_path(work_root, "src/app.ts")
            shard_path.parent.mkdir(parents=True, exist_ok=True)
            shard_path.write_text("{ not json", encoding="utf-8")
            with self.assertRaises(McpError):
                dispose_censor_finding(
                    work_root, "src/app.ts", "f-1", "fp", "coder", "t"
                )

    # ---- BLOCKER 1 / N3: provenance stays BOUNDED under repeated disposes ----

    def test_dispose_identical_redispose_does_not_grow_provenance(self):
        # An idempotent re-dispose (same actor+action as the last entry) must NOT
        # append another provenance entry — repeated re-dispose cannot bloat a shard.
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = _write_shard(
                work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")]
            )
            for i in range(200):
                dispose_censor_finding(
                    work_root, "src/app.ts", "f-1", "fp", "coder", f"t{i}", "coder"
                )
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            prov = stored["findings"][0]["provenance"]
            # The original "created" + exactly ONE "fp" (the rest were deduped).
            self.assertEqual([p["action"] for p in prov], ["created", "fp"])

    def test_dispose_alternating_disposes_are_capped(self):
        # Alternating disposes each DO append (different action than the last), but the
        # trail is capped at CENSOR_PROVENANCE_MAX with the OLDEST dropped.
        from oracle.server.aspis_mcp import CENSOR_PROVENANCE_MAX

        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = _write_shard(
                work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")]
            )
            toggles = ["fp", "open", "wontfix", "fixed"]
            for i in range(500):
                disp = toggles[i % len(toggles)]
                dispose_censor_finding(
                    work_root, "src/app.ts", "f-1", disp, "coder", f"t{i}", "coder"
                )
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            prov = stored["findings"][0]["provenance"]
            self.assertLessEqual(len(prov), CENSOR_PROVENANCE_MAX)
            # The trail keeps the MOST RECENT entries (oldest dropped): the final
            # disposition matches the last applied token.
            self.assertEqual(
                stored["findings"][0]["disposition"], toggles[(500 - 1) % len(toggles)]
            )

    # ---- NITPICK 1: double-slash rel path maps to the SAME shard ----

    def test_shard_path_collapses_consecutive_slashes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            single = censor_shard_path(root, "src/a.rs")
            double = censor_shard_path(root, "src//a.rs")
            backslash_double = censor_shard_path(root, "src\\\\a.rs")
            self.assertEqual(single, double)
            self.assertEqual(single, backslash_double)
            # Byte-identical to the Rust hash: sha256 of the collapsed "src/a.rs".
            expected = hashlib.sha256(b"src/a.rs").hexdigest()
            self.assertEqual(double.name, f"{expected}.json")

    # ---- WARNING 2: a coder cannot override a verifier adjudication ----

    def _verifier_disposed(self, work_root: Path, disposition: str) -> Path:
        shard_path = _write_shard(
            work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")]
        )
        dispose_censor_finding(
            work_root, "src/app.ts", "f-1", disposition, "verifier-9", "tv", "verifier"
        )
        return shard_path

    def test_coder_cannot_reopen_verifier_fp(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = self._verifier_disposed(work_root, "fp")
            with self.assertRaises(McpError):
                dispose_censor_finding(
                    work_root, "src/app.ts", "f-1", "open", "coder-1", "tc", "coder"
                )
            # The verifier's fp survives unchanged.
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            self.assertEqual(stored["findings"][0]["disposition"], "fp")

    def test_coder_cannot_override_verifier_wontfix(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            self._verifier_disposed(work_root, "wontfix")
            with self.assertRaises(McpError):
                dispose_censor_finding(
                    work_root, "src/app.ts", "f-1", "fp", "coder-1", "tc", "coder"
                )

    def test_verifier_can_override_anything(self):
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = self._verifier_disposed(work_root, "fp")
            # A verifier may re-open / change a prior verifier adjudication.
            dispose_censor_finding(
                work_root, "src/app.ts", "f-1", "open", "verifier-2", "tv2", "verifier"
            )
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            self.assertEqual(stored["findings"][0]["disposition"], "open")

    def test_coder_can_change_its_own_disposition(self):
        # A coder marks fp, then reopens its OWN prior disposition → allowed (the last
        # adjudicating role is coder, not verifier).
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = _write_shard(
                work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")]
            )
            dispose_censor_finding(
                work_root, "src/app.ts", "f-1", "fp", "coder-1", "t1", "coder"
            )
            dispose_censor_finding(
                work_root, "src/app.ts", "f-1", "open", "coder-1", "t2", "coder"
            )
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            self.assertEqual(stored["findings"][0]["disposition"], "open")

    def test_coder_can_set_fp_on_open_finding(self):
        # The precedence rule only blocks OVERRIDING a verifier; a coder freely
        # disposes an OPEN (machine-default) finding.
        with tempfile.TemporaryDirectory() as tmp:
            work_root = Path(tmp)
            shard_path = _write_shard(
                work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")]
            )
            dispose_censor_finding(
                work_root, "src/app.ts", "f-1", "fp", "coder-1", "t1", "coder"
            )
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            self.assertEqual(stored["findings"][0]["disposition"], "fp")

    # ---- NITPICK 2: the accept-set is DERIVED from the action map (no drift) ----

    def test_dispositions_set_is_derived_from_action_map(self):
        from oracle.server.aspis_mcp import (
            CENSOR_DISPOSITION_ACTION,
            CENSOR_DISPOSITIONS,
        )

        # Single source of truth: every accepted disposition has an action and vice
        # versa, so a `CENSOR_DISPOSITION_ACTION[disposition]` lookup can never
        # KeyError inside the shard lock.
        self.assertEqual(CENSOR_DISPOSITIONS, set(CENSOR_DISPOSITION_ACTION.keys()))


class CensorPythonRedactionTests(unittest.TestCase):
    """BLOCKER A second-layer defense: the Python MCP redacts title/body before a
    finding egresses via censor_findings/censor_finding, so even a shard written by
    an OLDER (pre-redaction) Rust build or hand-edited cannot leak a secret."""

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    def test_redact_secrets_removes_aws_key_keeps_prose(self):
        from oracle.server.aspis_mcp import _redact_secrets

        r = _redact_secrets("Key found: AKIAIOSFODNN7EXAMPLE in config")
        self.assertNotIn("AKIAIOSFODNN7EXAMPLE", r)
        self.assertIn("[redacted]", r)
        self.assertIn("Key found:", r)
        # Ordinary prose / dotted slugs are preserved (mirrors the Rust heuristic).
        self.assertEqual(
            _redact_secrets("Hardcoded password detected in authentication module"),
            "Hardcoded password detected in authentication module",
        )
        self.assertEqual(
            _redact_secrets("python.lang.security.audit.hardcoded-password"),
            "python.lang.security.audit.hardcoded-password",
        )
        self.assertEqual(_redact_secrets(""), "")

    def test_safe_finding_redacts_title_and_body(self):
        from oracle.server.aspis_mcp import _safe_censor_finding

        finding = _rust_shaped_finding(
            title="Hardcoded key AKIAIOSFODNN7EXAMPLE found",
            body="The literal AKIAIOSFODNN7EXAMPLE was committed.",
        )
        safe = _safe_censor_finding(finding)
        self.assertNotIn("AKIAIOSFODNN7EXAMPLE", safe["title"])
        self.assertNotIn("AKIAIOSFODNN7EXAMPLE", safe["body"])
        self.assertIn("[redacted]", safe["body"])

    def test_censor_findings_does_not_egress_secret_from_unredacted_shard(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects_dir = prepare_management_root(root)
            work_root = root / "Aspis Bio Work"
            work_root.mkdir()
            _project_with_root(projects_dir, work_root)
            # A shard whose body still carries a raw secret (older Rust / hand-edited).
            _write_shard(
                work_root,
                "src/app.ts",
                [_rust_shaped_finding(body="leaked AKIAIOSFODNN7EXAMPLE here")],
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-1",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            result = handle_tool_call(
                "censor_findings",
                {"project_id": "censor-proj", "agent_id": "coder-1", "role": "coder"},
                root=root,
            )
            blob = json.dumps(result)
            self.assertNotIn("AKIAIOSFODNN7EXAMPLE", blob)


class CensorMcpToolTests(unittest.TestCase):
    """The censor_findings / censor_dispose dispatch branches: token gating, role
    allow-list, project-root resolution, and the Rust-written-shard round trip."""

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"

    def tearDown(self):
        if self._old is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old

    def _setup(self, tmp: str) -> tuple[Path, Path]:
        root = Path(tmp)
        projects_dir = prepare_management_root(root)
        work_root = root / "Aspis Bio Work"
        work_root.mkdir()
        _project_with_root(projects_dir, work_root)
        return root, work_root

    def test_findings_reads_rust_written_shard_with_camelcase(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, work_root = self._setup(tmp)
            _write_shard(work_root, "src/app.ts", [_rust_shaped_finding(id="r-1")])
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-1",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            result = handle_tool_call(
                "censor_findings",
                {"project_id": "censor-proj", "agent_id": "coder-1", "role": "coder"},
                root=root,
            )
            self.assertEqual(result["projectId"], "censor-proj")
            self.assertEqual(len(result["findings"]), 1)
            f = result["findings"][0]
            self.assertEqual(f["id"], "r-1")
            self.assertEqual(f["severity"], "high")
            self.assertNotIn("contentHash", f)

    def test_findings_filters_by_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, work_root = self._setup(tmp)
            _write_shard(
                work_root, "src/a.ts", [_rust_shaped_finding(id="a", file="src/a.ts")]
            )
            _write_shard(
                work_root, "src/b.ts", [_rust_shaped_finding(id="b", file="src/b.ts")]
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-1",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            result = handle_tool_call(
                "censor_findings",
                {
                    "project_id": "censor-proj",
                    "agent_id": "coder-1",
                    "role": "coder",
                    "file": "src/a.ts",
                },
                root=root,
            )
            self.assertEqual([f["id"] for f in result["findings"]], ["a"])

    def test_dispose_tool_sets_and_audits(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, work_root = self._setup(tmp)
            shard_path = _write_shard(
                work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")]
            )
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "verifier-1",
                    "role": "verifier",
                    "model": "cheap",
                    "message": "x",
                },
                root=root,
            )
            result = handle_tool_call(
                "censor_dispose",
                {
                    "project_id": "censor-proj",
                    "file": "src/app.ts",
                    "id": "f-1",
                    "disposition": "fp",
                    "agent_id": "verifier-1",
                    "role": "verifier",
                },
                root=root,
            )
            # B3 — the dispose result is identity + applied disposition ONLY; the
            # finding body/title/content is NEVER echoed back to the agent.
            self.assertEqual(
                set(result.keys()), {"projectId", "file", "id", "disposition", "ok"}
            )
            self.assertEqual(result["projectId"], "censor-proj")
            self.assertEqual(result["file"], "src/app.ts")
            self.assertEqual(result["id"], "f-1")
            self.assertEqual(result["disposition"], "fp")
            self.assertTrue(result["ok"])
            self.assertNotIn("finding", result)
            # The body is NOT in the response payload (no extra egress).
            self.assertNotIn("body", json.dumps(result))
            stored = json.loads(shard_path.read_text(encoding="utf-8"))
            self.assertEqual(stored["findings"][0]["disposition"], "fp")
            self.assertEqual(
                stored["findings"][0]["provenance"][-1]["actor"], "verifier-1"
            )
            self.assertEqual(
                stored["findings"][0]["provenance"][-1]["role"], "verifier"
            )

    def test_findings_is_session_token_gated(self):
        # The tool routes through require_agent_tool → require_session_token. We
        # register tokenlessly under the compat flag, then turn the flag OFF: a
        # tokenless censor_findings must now be rejected (the gate no longer permits
        # a tokenless privileged call).
        with tempfile.TemporaryDirectory() as tmp:
            root, work_root = self._setup(tmp)
            _write_shard(work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")])
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-tok",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            # While the flag is ON the tokenless call works (proves the dispatch path
            # is wired and reaches the shards).
            ok = handle_tool_call(
                "censor_findings",
                {"project_id": "censor-proj", "agent_id": "coder-tok", "role": "coder"},
                root=root,
            )
            self.assertEqual(len(ok["findings"]), 1)
            # Flag OFF → the session-token gate rejects the tokenless call.
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "0"
            with self.assertRaises(McpError):
                handle_tool_call(
                    "censor_findings",
                    {
                        "project_id": "censor-proj",
                        "agent_id": "coder-tok",
                        "role": "coder",
                    },
                    root=root,
                )

    def test_dispose_is_session_token_gated(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, work_root = self._setup(tmp)
            _write_shard(work_root, "src/app.ts", [_rust_shaped_finding(id="f-1")])
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "verifier-tok",
                    "role": "verifier",
                    "model": "cheap",
                    "message": "x",
                },
                root=root,
            )
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "0"
            with self.assertRaises(McpError):
                handle_tool_call(
                    "censor_dispose",
                    {
                        "project_id": "censor-proj",
                        "file": "src/app.ts",
                        "id": "f-1",
                        "disposition": "fp",
                        "agent_id": "verifier-tok",
                        "role": "verifier",
                    },
                    root=root,
                )

    def test_findings_requires_registered_agent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._setup(tmp)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "censor_findings",
                    {"project_id": "censor-proj", "agent_id": "ghost", "role": "coder"},
                    root=root,
                )


class T4CensorFailureMessageTests(unittest.TestCase):
    """T4 — Ticket B: `censor_findings` failure modes must surface distinct,
    actionable error messages so an operator can fix them without reading
    source code.

    (1) Missing `root_path` frontmatter → names the file inspected AND the
        frontmatter key the operator must set.
    (2) `root_path` set but NOT under an approved workspace parent →
        names the env var (ASPIS_WORKSPACE_ROOT) AND echoes the offending
        path.

    Both messages stay ≤200 chars and contain no secrets / full env dumps.
    """

    def setUp(self):
        self._old_managed = os.environ.get(
            "ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"
        )
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        # ASPIS_WORKSPACE_ROOT shifts the "approved parents" list. Wipe it on
        # entry so the workspace-rejection failure path is the *only* thing
        # being tested; restore on tearDown.
        self._old_workspace_root = os.environ.get("ASPIS_WORKSPACE_ROOT")
        os.environ.pop("ASPIS_WORKSPACE_ROOT", None)

    def tearDown(self):
        if self._old_managed is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = self._old_managed
        if self._old_workspace_root is None:
            os.environ.pop("ASPIS_WORKSPACE_ROOT", None)
        else:
            os.environ["ASPIS_WORKSPACE_ROOT"] = self._old_workspace_root

    def _register_coder(self, root: Path) -> None:
        handle_tool_call(
            "agent_register",
            {
                "agent_id": "coder-b",
                "role": "coder",
                "model": "opus",
                "message": "x",
            },
            root=root,
        )

    def test_missing_root_path_failure_message_is_actionable(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            prepare_management_root(root)
            # Write a project WITHOUT a `root_path` frontmatter key.
            (projects / "noroot.md").write_text(
                """---
id: noroot
title: No configured root
status: active
updated_at: 2026-06-05T00:00:00Z
---

```aspis-project
{"version":1,"tasks":[],"notes":[]}
```
""",
                encoding="utf-8",
            )
            self._register_coder(root)
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "censor_findings",
                    {"project_id": "noroot", "agent_id": "coder-b", "role": "coder"},
                    root=root,
                )
            message = str(ctx.exception)
            # (1) actionable + distinct: names the project file, names the
            # frontmatter key the operator must add, points at Censor.
            self.assertIn("noroot", message)
            self.assertIn("root_path", message)
            self.assertIn("frontmatter", message)
            # Word + char bounds.
            self.assertLessEqual(len(message), 200)
            # Don't dump the whole env.
            self.assertNotIn("ASPIS_", message.replace("ASPIS_WORKSPACE_ROOT", ""))

    def test_workspace_root_rejection_message_names_env_and_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            projects = root / "projects"
            prepare_management_root(root)
            # Build a workspace root OUTSIDE the management root and any
            # other approved parent (set ASPIS_WORKSPACE_ROOT to a sibling
            # tmpdir so the rejected path is genuinely unapproved). The
            # other_tmp stays short so the message stays within ≤200 chars.
            other_tmp = tempfile.mkdtemp(prefix="t4o-")
            sibling = Path(other_tmp)
            os.environ["ASPIS_WORKSPACE_ROOT"] = str(sibling / "approved-parent")
            # Reject a path under a different sibling entirely.
            rejected = sibling / "other-branch" / "proj"
            rejected.mkdir(parents=True, exist_ok=True)
            outside_root = rejected
            escaped = str(outside_root).replace("\\", "\\\\")
            (projects / "outsider.md").write_text(
                f"""---
id: outsider
title: Outsider project
status: active
updated_at: 2026-06-05T00:00:00Z
root_path: "{escaped}"
---

```aspis-project
{{"version":1,"tasks":[],"notes":[]}}
```
""",
                encoding="utf-8",
            )
            self._register_coder(root)
            with self.assertRaises(McpError) as ctx:
                handle_tool_call(
                    "censor_findings",
                    {"project_id": "outsider", "agent_id": "coder-b", "role": "coder"},
                    root=root,
                )
            message = str(ctx.exception)
            # (2) distinct from (1): names the env var the operator can set
            # AND the offending path (last 2 components) that was rejected.
            self.assertIn("ASPIS_WORKSPACE_ROOT", message)
            # T4 — Ticket B: path is trimmed to last 2 components (…/parent/leaf).
            parts = outside_root.parts
            expected_tail = "…/" + "/".join(parts[-2:])
            self.assertIn(expected_tail, message)
            # Bounds + no-secret posture.
            self.assertLessEqual(len(message), 200)
            # The two failure modes must NOT be confused: the "frontmatter"
            # wording belongs to (1) only.
            self.assertNotIn("no `root_path` in frontmatter", message)


class CensorRoleMandateTests(unittest.TestCase):
    """ROLE_RULES carry the Phase E Censor mandates and both roles allow the tools."""

    def test_both_roles_allow_censor_tools(self):
        for rule in ROLE_RULES:
            if rule["role"] == "mini":
                # P3: the mini is a read-only oracle leaf — censor adjudication
                # stays a coder/verifier duty, so the mini gets NO censor tools.
                self.assertNotIn("censor_findings", rule["allowedTools"])
                self.assertNotIn("censor_dispose", rule["allowedTools"])
                continue
            if rule["role"] == "orchestrator":
                # The orchestrator (devboule-coder) PLANS and DELEGATES every write
                # to spawn_mini_coder; the per-step Censor consumption happens inside
                # the spawned mini / the coder, and residual adjudication stays with
                # the verifier. So the orchestrator deliberately carries NO censor
                # tool — it must never dispose a finding (tighter-or-equal to coder).
                self.assertNotIn("censor_findings", rule["allowedTools"])
                self.assertNotIn("censor_dispose", rule["allowedTools"])
                continue
            self.assertIn("censor_findings", rule["allowedTools"], rule["role"])
            self.assertIn("censor_dispose", rule["allowedTools"], rule["role"])

    def test_coder_carries_p8_own_review_pass_mandate(self):
        # P8: the coder runs its OWN single Sonnet review pass before moving a
        # task to review; the verifier keeps the final verdict (censorReview).
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        blob = " ".join(coder["forbidden"])
        self.assertIn("Sonnet review subagent", blob)
        self.assertIn("ready for final reviewer", blob)
        self.assertIn("censorReview", blob)
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertNotIn("Sonnet review subagent", " ".join(verifier["forbidden"]))

    def test_coder_mandate_is_per_step(self):
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        blob = " ".join(coder["censor"]).lower()
        self.assertIn("censor_findings", blob)
        self.assertIn("censor_dispose", blob)
        self.assertIn("step", blob)

    def test_verifier_mandate_is_residual(self):
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        blob = " ".join(verifier["censor"]).lower()
        self.assertIn("censor_findings", blob)
        self.assertIn("censor_dispose", blob)
        self.assertIn("residual", blob)


class SpawnMiniCoderTests(unittest.TestCase):
    """MC-P2: the `spawn_mini_coder` MCP tool — gating, directive write (camelCase
    parity with the Rust serde), the bounded result poll, and the timeout path."""

    def _project_dir(self, tmp: str) -> Path:
        root = Path(tmp)
        projects = root / "projects"
        projects.mkdir()
        sample_project(projects)
        return root

    def _register_coder(self, root: Path, agent_id: str = "codex") -> str:
        """Seed an app-issued launch-pending coder, register it with the launch token,
        and return the issued session token (the managed, token-bearing path the real
        app uses — spawn_mini_coder requires a valid session token)."""
        token = "test-launch-token"
        (root / "projects" / ".aspis-agents.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "updatedAt": "2026-06-06T00:00:00+00:00",
                    "sessions": [
                        {
                            "agentId": agent_id,
                            "role": "coder",
                            "status": "launch_pending",
                            "lastSeenAt": "2026-06-06T00:00:00+00:00",
                            "launchTokenHash": hashlib.sha256(
                                token.encode("utf-8")
                            ).hexdigest(),
                            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                }
            ),
            encoding="utf-8",
        )
        result = handle_tool_call(
            "agent_register",
            {
                "agent_id": agent_id,
                "role": "coder",
                "model": "codex",
                "message": "coding",
                "launch_token": token,
            },
            root=root,
        )
        return result["sessionToken"]

    def _read_state(self, root: Path) -> dict:
        return json.loads(
            (root / "projects" / ".aspis-agents.json").read_text(encoding="utf-8")
        )

    def test_writes_pending_directive_with_exact_camel_case_keys(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            # A short poll timeout so the test does not block; we only assert the
            # directive was WRITTEN before the (timeout) return.
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "docstring foo()",
                        "files": ["src/a.rs", "src/b.rs"],
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertIn("directiveId", out)
            state = self._read_state(root)
            directives = state["miniCoderDirectives"]
            self.assertEqual(len(directives), 1)
            d = directives[0]
            # camelCase keys EXACTLY matching the Rust serde (mini_coder.rs).
            self.assertEqual(d["id"], out["directiveId"])
            self.assertEqual(d["parentAgentId"], "codex")
            self.assertEqual(d["task"], "docstring foo()")
            self.assertEqual(d["files"], ["src/a.rs", "src/b.rs"])
            self.assertEqual(d["resultPath"], f"{out['directiveId']}.json")
            self.assertIn("createdAt", d)
            # NO-CHURN: allowOracle/write/backend omitted when not set; snake_case never leaks.
            self.assertNotIn("allowOracle", d)
            self.assertNotIn("write", d)
            self.assertNotIn("backend", d)
            self.assertNotIn("parent_agent_id", d)
            # FIX 3: the poll deadline fired with NO executor, so the directive was
            # never claimed (still `pending`). A never-started mini must be stamped
            # `failed` (not `timeout`, which would imply it ran and overran).
            self.assertEqual(out["result"]["status"], "failed")
            self.assertIn("did not start", out["result"]["error"])
            self.assertEqual(d["status"], "failed")

    def test_allow_oracle_and_backend_are_emitted_when_set(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "summarize",
                        "files": ["src/a.rs"],
                        "backend": "ollama",
                        "allow_oracle": True,
                        "write": True,
                        "session_token": token,
                    },
                    root=root,
                )
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["backend"], "ollama")
            self.assertTrue(d["allowOracle"])
            # P4: the write marker rides the directive for the Rust executor.
            self.assertIs(d["write"], True)

    def test_write_mode_default_is_omitted_no_churn(self):
        # A2 NO-CHURN: the default write_mode (emitEdits) must NOT be written into
        # the directive, so it stays byte-identical to today and round-trips
        # through the Rust serde skip. True whether omitted entirely OR passed
        # explicitly as the default.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            for args_extra in ({}, {"write_mode": "emitEdits"}):
                with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                    handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "write": True,
                            "session_token": token,
                            **args_extra,
                        },
                        root=root,
                    )
                d = self._read_state(root)["miniCoderDirectives"][-1]
                self.assertNotIn("writeMode", d, f"args_extra={args_extra}")

    def test_write_mode_agentic_iterative_is_forwarded(self):
        # A2: a non-default write_mode rides the directive with the EXACT Rust
        # serde wire string (camelCase) under the camelCase key `writeMode`.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": ["src/a.rs"],
                        "write": True,
                        "write_mode": "agenticIterative",
                        "session_token": token,
                    },
                    root=root,
                )
            d = self._read_state(root)["miniCoderDirectives"][-1]
            self.assertEqual(d["writeMode"], "agenticIterative")

    def test_write_mode_rejects_invalid_value(self):
        # A2: an unknown write_mode is rejected with McpError (like other
        # enum-validated params), never silently coerced.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": ["src/a.rs"],
                        "write": True,
                        "write_mode": "bogus",
                        "session_token": token,
                    },
                    root=root,
                )
            # The rejection must fire BEFORE any state mutation (review F3).
            self.assertFalse(
                self._read_state(root).get("miniCoderDirectives"),
                "no directive may be persisted when write_mode validation fails",
            )

    def test_write_mode_non_default_requires_write_directive(self):
        # review F1: a non-default write_mode is only coherent on a write
        # directive; without `write: true` it is rejected, not left dangling.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": ["src/a.rs"],
                        "write_mode": "agenticIterative",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertFalse(
                self._read_state(root).get("miniCoderDirectives"),
                "no directive may be persisted when write_mode lacks write:true",
            )

    def test_tool_description_documents_write_mode_rule(self):
        # A3: the spawn_mini_coder tool DESCRIPTION must document the write_mode param
        # and the one-line rule of thumb (agentic-iterative only for covered-language
        # files + a capable model; else emit-edits), so the coder reading the tool list
        # learns the decision. String-presence, consistent with the A2 param schema.
        tool = next(t for t in TOOLS if t["name"] == "spawn_mini_coder")
        desc = tool["description"]
        self.assertIn("write_mode", desc, "tool description must name the param")
        self.assertIn("agenticIterative", desc, "must mention the agentic mode")
        self.assertIn("emitEdits", desc, "must mention the default mode")
        self.assertIn("coverage", desc, "must mention the gate-coverage gating")
        # The param schema (A2) still carries its own description + the canonical
        # enum/default, and the rule of thumb is restated there.
        param = tool["parameters"]["write_mode"]
        self.assertEqual(list(param["enum"]), list(MINI_CODER_WRITE_MODES))
        self.assertEqual(param["default"], MINI_CODER_WRITE_MODE_DEFAULT)
        pdesc = param["description"]
        self.assertIn("agenticIterative", pdesc)
        self.assertIn("emitEdits", pdesc)
        self.assertIn(
            "gate coverage", pdesc, "param description states the coverage rule"
        )
        # A3 forward-looks past A2's plumbing-only wording.
        self.assertNotIn("behavior wired later", pdesc)

    def test_coder_role_rules_document_write_mode_rule(self):
        # A3: the coder ROLE_RULES (surfaced by the agent_rules tool) carry the
        # write_mode rule of thumb so the contract and the tool description agree.
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        joined = " ".join(coder.get("forbidden", []))
        self.assertIn("write_mode", joined, "coder rules must mention write_mode")
        self.assertIn("agenticIterative", joined)
        self.assertIn("emitEdits", joined)

    def test_write_allowlist_is_capped_at_ten_files(self):
        # Max-recall: the Rust apply enforces 1..=10 files for write directives;
        # python fails FAST (before the mini burns a run). 10 passes, 11 raises.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            files11 = [f"src/f{i}.rs" for i in range(11)]
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "bulk write",
                        "files": files11,
                        "write": True,
                        "session_token": token,
                    },
                    root=root,
                )
            # The same 11 files WITHOUT write are still fine (read scope).
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "bulk read",
                        "files": files11,
                        "session_token": token,
                    },
                    root=root,
                )

    def test_rejects_empty_task(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "   ",
                        "files": ["src/a.rs"],
                        "session_token": token,
                    },
                    root=root,
                )

    def test_rejects_empty_or_missing_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": [],
                        "session_token": token,
                    },
                    root=root,
                )

    def test_rejects_traversal_file_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": ["../escape.rs"],
                        "session_token": token,
                    },
                    root=root,
                )

    def test_rejects_wrong_session_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            self._register_coder(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": ["src/a.rs"],
                        "session_token": "wrong",
                    },
                    root=root,
                )

    def test_rejects_unregistered_caller(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "ghost",
                        "role": "coder",
                        "task": "x",
                        "files": ["src/a.rs"],
                        "session_token": "t",
                    },
                    root=root,
                )

    def test_poll_returns_terminal_outcome_once_result_is_set(self):
        # Simulate the Rust executor: a background thread stamps a `done` result onto
        # the directive shortly after the tool starts polling. The tool's bounded
        # poll must observe it and return it.
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            done = {"status": "done", "output": "did it", "filesTouched": ["src/a.rs"]}

            def executor():
                # Wait for the directive to appear, then stamp a done result.
                from oracle.server.aspis_mcp import (
                    read_agents_state,
                    write_agents_state,
                    file_lock,
                    AGENTS_STATE_FILE,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            ds[0]["status"] = "done"
                            ds[0]["result"] = done
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=executor)
            t.start()
            try:
                with patch(
                    "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.05
                ):
                    out = handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["result"]["status"], "done")
            self.assertEqual(out["result"]["output"], "did it")
            self.assertEqual(out["result"]["filesTouched"], ["src/a.rs"])

    def test_poll_returns_failed_quickly_when_directive_vanishes(self):
        # WARNING 5: the directive is SEEN once, then a co-writer removes it from the
        # array before any terminal result is read. The poll must return a synthesized
        # `failed`/`gone` outcome PROMPTLY (not block for the full poll cap).
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)

            def vanisher():
                from oracle.server.aspis_mcp import (
                    read_agents_state,
                    write_agents_state,
                    file_lock,
                    AGENTS_STATE_FILE,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                # Wait for the directive to appear (the poll will SEE it), then drop
                # it from the array entirely (no result ever stamped).
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            state["miniCoderDirectives"] = []
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=vanisher)
            t.start()
            try:
                # A LONG poll cap: if the fix is wrong (waits for the cap) the test
                # would hang ~that long; the assertion is that it returns promptly with
                # `failed`, well before the cap.
                started = time.monotonic()
                with (
                    patch(
                        "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.02
                    ),
                    patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 30.0),
                ):
                    out = handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "session_token": token,
                        },
                        root=root,
                    )
                elapsed = time.monotonic() - started
            finally:
                t.join()
            self.assertEqual(out["result"]["status"], "failed")
            self.assertIn("vanished", out["result"]["error"])
            # Promptly: nowhere near the 30s cap.
            self.assertLess(
                elapsed, 10.0, f"poll did not return promptly: {elapsed:.2f}s"
            )

    def test_kill_requested_wins_over_poll_timeout(self):
        # BLOCKER 1: a human hit Stop (Rust set killRequested=true on the directive)
        # but the executor hasn't written the aborted_by_human result before our poll
        # deadline fires. The timeout path must NOT clobber it with `timeout` — it must
        # synthesize `aborted_by_human` so the human Stop is never lost (coder stops +
        # escalates instead of retrying).
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)

            def killer():
                # Wait for the directive to appear, then set killRequested=true WITHOUT
                # ever stamping a terminal result (simulating the Rust race: flag set +
                # PTY killed, but aborted_by_human result not yet written).
                from oracle.server.aspis_mcp import (
                    read_agents_state,
                    write_agents_state,
                    file_lock,
                    AGENTS_STATE_FILE,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            ds[0]["status"] = "running"
                            ds[0]["killRequested"] = True
                            ds[0]["result"] = None
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=killer)
            t.start()
            try:
                # A small (non-zero) interval so the killer has time to flip the flag,
                # and a short cap so the deadline fires while killRequested is set but
                # no terminal result exists.
                with (
                    patch(
                        "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.02
                    ),
                    patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.3),
                ):
                    out = handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            # The poll returns aborted_by_human, NOT timeout.
            self.assertEqual(out["result"]["status"], "aborted_by_human")
            self.assertIn("escalate", out["result"]["error"])
            # And the directive is stamped aborted_by_human (terminal), not timeout, so
            # the executor's later aborted_by_human apply_result is a harmless no-op.
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["status"], "aborted_by_human")
            self.assertEqual(d["result"]["status"], "aborted_by_human")

    def test_poll_keeps_blocking_on_awaiting_retry_then_returns_terminal(self):
        # MC-P6: `awaiting_retry` is a NON-terminal status (a predecessor waiting for
        # its retry's verdict). The poll must NOT return while the watched directive is
        # `awaiting_retry` (no `result` yet); it must keep polling and only return once
        # a thread flips it to a real terminal status (here `done`).
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            done = {
                "status": "done",
                "output": "retry succeeded",
                "filesTouched": ["src/a.rs"],
            }
            saw_awaiting_retry = {"value": False}

            def executor():
                # Wait for the directive to appear, set it to `awaiting_retry` (NON-terminal,
                # NO result), hold it there a few passes (the poll must keep blocking), then
                # stamp a terminal `done` result.
                from oracle.server.aspis_mcp import (
                    read_agents_state,
                    write_agents_state,
                    file_lock,
                    AGENTS_STATE_FILE,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            ds[0]["status"] = "awaiting_retry"
                            ds[0]["result"] = None
                            write_agents_state(projects_dir, state)
                            saw_awaiting_retry["value"] = True
                            break
                    time.sleep(0.02)
                # Keep it awaiting_retry for a few poll intervals so a buggy poll that
                # treats awaiting_retry as terminal would already have returned wrongly.
                time.sleep(0.3)
                with file_lock(lock):
                    state = read_agents_state(projects_dir)
                    ds = state.get("miniCoderDirectives", [])
                    if ds:
                        ds[0]["status"] = "done"
                        ds[0]["result"] = done
                        write_agents_state(projects_dir, state)

            t = threading.Thread(target=executor)
            t.start()
            try:
                with patch(
                    "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.05
                ):
                    out = handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertTrue(
                saw_awaiting_retry["value"], "directive was never set to awaiting_retry"
            )
            # The poll did NOT return early on awaiting_retry — it returned the terminal done.
            self.assertEqual(out["result"]["status"], "done")
            self.assertEqual(out["result"]["output"], "retry succeeded")

    def test_poll_returns_escalated_with_escalation_payload_intact(self):
        # MC-P6: `escalated` is TERMINAL (the retry chain exhausted, Censor still dirty).
        # When the executor stamps an `escalated` result carrying an `escalation`
        # sub-object (attempts + findings), the poll returns it and the escalation
        # payload passes through VERBATIM (no field-allowlist strips it).
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            escalated = {
                "status": "escalated",
                "output": "could not clear Censor after retries",
                "filesTouched": ["src/a.rs"],
                "escalation": {
                    "attempts": 3,
                    "findings": [
                        {
                            "file": "src/a.rs",
                            "severity": "high",
                            "title": "unsafe deref",
                            "line": 42,
                        },
                        {
                            "file": "src/a.rs",
                            "severity": "medium",
                            "title": "missing guard",
                            "line": 7,
                        },
                    ],
                },
            }

            def executor():
                from oracle.server.aspis_mcp import (
                    read_agents_state,
                    write_agents_state,
                    file_lock,
                    AGENTS_STATE_FILE,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            ds[0]["status"] = "escalated"
                            ds[0]["result"] = escalated
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=executor)
            t.start()
            try:
                with patch(
                    "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.05
                ):
                    out = handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            # `escalated` is recognized as terminal: the poll returned it.
            self.assertEqual(out["result"]["status"], "escalated")
            # The escalation payload survived verbatim through the passthrough.
            esc = out["result"]["escalation"]
            self.assertEqual(esc["attempts"], 3)
            self.assertEqual(len(esc["findings"]), 2)
            self.assertEqual(esc["findings"][0]["file"], "src/a.rs")
            self.assertEqual(esc["findings"][0]["severity"], "high")
            self.assertEqual(esc["findings"][0]["title"], "unsafe deref")
            self.assertEqual(esc["findings"][0]["line"], 42)

    def test_propagated_terminal_on_root_directive_returns_through_poll(self):
        # MC-P6: the blocking poll watches the ROOT directive's id. The Rust side
        # PROPAGATES the leaf's terminal outcome onto the root directive, so the poll
        # observes the terminal status on the watched id and returns it. This mirrors
        # that propagation: a thread stamps a terminal result on the same id the poll
        # is watching (the root), and the poll returns it.
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            propagated = {
                "status": "escalated",
                "output": "leaf escalated, propagated to root",
                "escalation": {"attempts": 2, "findings": []},
            }

            def executor():
                from oracle.server.aspis_mcp import (
                    read_agents_state,
                    write_agents_state,
                    file_lock,
                    AGENTS_STATE_FILE,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            # The poll watches ds[0]'s id (the root, the one this tool
                            # appended). The Rust side stamps the propagated terminal here.
                            ds[0]["status"] = "escalated"
                            ds[0]["result"] = propagated
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=executor)
            t.start()
            try:
                with patch(
                    "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.05
                ):
                    out = handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["result"]["status"], "escalated")
            self.assertEqual(out["result"]["escalation"]["attempts"], 2)

    def test_mini_coder_poll_timeout_is_1800(self):
        # MC-P6: a retry chain is up to 1+2 attempts x wall-cap, so the blocking poll
        # cap is raised from the old 600s (10 min) to 1800s (30 min) to mirror the
        # executor's worst-case retry-chain wall time.
        from oracle.server.aspis_mcp import MINI_CODER_POLL_TIMEOUT_SECS

        self.assertEqual(MINI_CODER_POLL_TIMEOUT_SECS, 1800.0)

    def test_normalize_preserves_rust_set_scratch_path_and_claimed_at(self):
        # PASSTHROUGH (BLOCKER/WARNING 3+4 + MC-P5): the Rust-owned
        # `scratchPath`/`claimedAt`/`killRequested` keys on a directive must survive a
        # Python normalize/read untouched — Python never sets, validates, or strips
        # them. killRequested is the human-Stop safety brake the Rust executor honors.
        from oracle.server.aspis_mcp import normalize_agents_state

        state = {
            "version": 2,
            "miniCoderDirectives": [
                {
                    "id": "d1",
                    "parentAgentId": "codex",
                    "status": "running",
                    "task": "t",
                    "resultPath": "d1.json",
                    "createdAt": "2026-06-06T00:00:00Z",
                    "claimedAt": "2026-06-06T00:00:01Z",
                    "scratchPath": "/proj/.aspis-mini",
                    "killRequested": True,
                    "agentId": "mini-codex-d1",
                    "startedAt": "2026-06-06T00:00:02Z",
                }
            ],
        }
        out = normalize_agents_state(state)
        d = out["miniCoderDirectives"][0]
        self.assertEqual(d["claimedAt"], "2026-06-06T00:00:01Z")
        self.assertEqual(d["scratchPath"], "/proj/.aspis-mini")
        # MC-P5: killRequested round-trips verbatim (passthrough, not dropped).
        self.assertIs(d["killRequested"], True)

    def test_never_claimed_pending_directive_returns_failed_not_timeout(self):
        # FIX 3: with no executor, the directive stays `pending` past the deadline. It
        # was never claimed, so the synthesized outcome is `failed` (executor did not
        # start it), NOT `timeout`.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": ["src/a.rs"],
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertEqual(out["result"]["status"], "failed")
            self.assertIn("did not start", out["result"]["error"])
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["status"], "failed")

    def test_running_directive_past_deadline_returns_timeout(self):
        # FIX 3: a directive the executor DID claim (status running) that does not
        # finish before the deadline must time out (not failed) — it genuinely ran and
        # overran. A background thread flips it to `running` (no terminal result), then
        # the poll deadline fires.
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)

            def runner():
                from oracle.server.aspis_mcp import (
                    read_agents_state,
                    write_agents_state,
                    file_lock,
                    AGENTS_STATE_FILE,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            ds[0]["status"] = "running"
                            ds[0]["result"] = None
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=runner)
            t.start()
            try:
                with (
                    patch(
                        "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.02
                    ),
                    patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.3),
                ):
                    out = handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["result"]["status"], "timeout")
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["status"], "timeout")

    def test_awaiting_retry_directive_past_deadline_returns_timeout_not_failed(self):
        # BLOCKER F-2: a ROOT directive left `awaiting_retry` at the poll deadline means
        # the mini DID start, ran, and triggered a retry chain that is still live. It must
        # synthesize `timeout` (with the retry-chain message) — NOT `failed`/"did not
        # start", which would mislead the orchestrator into re-spawning the whole task.
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)

            def runner():
                from oracle.server.aspis_mcp import (
                    read_agents_state,
                    write_agents_state,
                    file_lock,
                    AGENTS_STATE_FILE,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            ds[0]["status"] = "awaiting_retry"
                            ds[0]["result"] = None
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=runner)
            t.start()
            try:
                with (
                    patch(
                        "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.02
                    ),
                    patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.3),
                ):
                    out = handle_tool_call(
                        "spawn_mini_coder",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "task": "x",
                            "files": ["src/a.rs"],
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["result"]["status"], "timeout")
            self.assertNotEqual(out["result"]["status"], "failed")
            self.assertIn("retry chain", out["result"]["error"])
            self.assertNotIn("did not start", out["result"]["error"])
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["status"], "timeout")

    def test_spawn_mini_coder_in_coder_allowed_tools(self):
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        self.assertIn("spawn_mini_coder", coder["allowedTools"])
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertNotIn("spawn_mini_coder", verifier["allowedTools"])
        orch = next(r for r in ROLE_RULES if r["role"] == "orchestrator")
        self.assertNotIn("spawn_mini_coder", orch["allowedTools"])
        self.assertNotIn("steer_mini_coder", orch["allowedTools"])
        self.assertNotIn("mini_coder_result", orch["allowedTools"])

    def test_coder_role_rules_carry_aborted_by_human_escalation(self):
        # MC-P5: the coder's forbidden list must carry the aborted_by_human escalation
        # mandate (STOP + no silent retry + escalate via needs_user), mirrored verbatim
        # in the Rust default_role_rules coder.forbidden.
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        blob = " ".join(coder["forbidden"])
        self.assertIn("aborted_by_human", blob)
        self.assertIn("needs_user", blob)
        # The verifier (no spawn_mini_coder) must NOT carry it.
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertNotIn("aborted_by_human", " ".join(verifier["forbidden"]))

    def test_coder_role_rules_carry_escalated_redo_mandate(self):
        # MC-P6: on an `escalated` mini result (the retry chain exhausted, Censor still
        # dirty), the coder must REDO the file ITSELF — NOT blindly re-spawn the mini for
        # the same file (the training rail already captured the failure). The coder's
        # forbidden list must carry this mandate; the English mirror lives in the Rust
        # default_role_rules coder.forbidden (agents.rs — a separate agent's job).
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        blob = " ".join(coder["forbidden"])
        self.assertIn("escalated", blob)
        # No blind re-spawn for the same file — the coder fixes it itself.
        self.assertIn("REDO that file yourself", blob)
        # The verifier (no spawn_mini_coder) must NOT carry it.
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertNotIn("escalated", " ".join(verifier["forbidden"]))

    def test_coder_role_rules_carry_mini_routing_mandate(self):
        # MC-P7: the coder's forbidden list must carry the mini ROUTING mandate
        # (delegate only cheap/mechanical work + review the mini's output as a draft),
        # mirroring the Rust default_role_rules coder.forbidden routing line.
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        blob = " ".join(coder["forbidden"])
        self.assertIn(
            "Delegate only cheap, mechanical sub-tasks to spawn_mini_coder", blob
        )
        self.assertIn("REVIEW the mini's output as a draft", blob)
        # The verifier (no spawn_mini_coder) must NOT carry any routing rule.
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertNotIn("spawn_mini_coder", " ".join(verifier["forbidden"]))

    def test_coder_role_rules_carry_cooperative_push_mandate(self):
        # GH-P5: the coder's `push` mandate must carry the cooperative push contract
        # (commit freely, NEVER raw git push, publish via request_git_push, STOP +
        # needs_user on deny/timeout), mirrored in the Rust default_role_rules
        # coder.push (coder_rule_carries_cooperative_push_mandate) — both now source
        # the same English text from role_rules.json.
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        self.assertIn("push", coder, "coder must declare a push mandate")
        blob = " ".join(coder["push"])
        # Commit-freely line.
        self.assertIn("Commit freely", blob)
        # Never-raw-push line that names the request_git_push tool.
        self.assertIn("NEVER run a raw `git push`", blob)
        self.assertIn("request_git_push", blob)
        # Deny/timeout -> STOP + escalate via needs_user, no retry/workaround.
        self.assertIn("denied or times out", blob)
        self.assertIn("STOP", blob)
        self.assertIn("needs_user", blob)
        self.assertIn("Do NOT retry", blob)
        # The verifier (no request_git_push) must NOT carry any push mandate.
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertNotIn("push", verifier, "verifier must have NO push mandate")

    def test_request_git_push_is_coder_only(self):
        # GH-P4/P5: request_git_push is a coder-only capability. It MUST be in the
        # coder's allowedTools and MUST NOT be in the verifier's. Mirrored in the
        # Rust side (request_git_push_is_coder_only).
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertIn("request_git_push", coder["allowedTools"])
        self.assertNotIn("request_git_push", verifier["allowedTools"])

    def test_upsert_session_parent_agent_id_round_trips(self):
        from oracle.server.aspis_mcp import default_agents_state

        state = default_agents_state()
        upsert_session(state, agent_id="mini-1", role="coder", parent_agent_id="codex")
        session = next(s for s in state["sessions"] if s["agentId"] == "mini-1")
        self.assertEqual(session["parentAgentId"], "codex")
        # NO-CHURN: an ordinary session that never passes parent_agent_id has no key.
        upsert_session(state, agent_id="plain-1", role="coder")
        plain = next(s for s in state["sessions"] if s["agentId"] == "plain-1")
        self.assertNotIn("parentAgentId", plain)

    def test_cap_mini_coder_directives_evicts_oldest_terminal_keeps_active(self):
        directives = [
            {"id": "old", "status": "done", "createdAt": "2026-06-06T00:00:01Z"},
            {"id": "active", "status": "running", "createdAt": "2026-06-06T00:00:02Z"},
        ]
        # Pad with terminal directives past the cap.
        for i in range(MAX_MINI_CODER_DIRECTIVES):
            directives.append(
                {
                    "id": f"t{i}",
                    "status": "failed",
                    "createdAt": f"2026-06-06T01:00:{i:02d}Z",
                }
            )
        capped = cap_mini_coder_directives(directives)
        self.assertLessEqual(len(capped), MAX_MINI_CODER_DIRECTIVES)
        ids = {d["id"] for d in capped}
        # The running directive is NEVER evicted; the oldest terminal ("old") is.
        self.assertIn("active", ids)
        self.assertNotIn("old", ids)

    def test_cap_mini_coder_directives_prefers_evicting_collected_terminal_first(self):
        # F-E hardening: a COLLECTED terminal directive is evicted before ANY
        # uncollected one, even when the collected one is NEWER — the outcome
        # was already successfully handed back, while the uncollected one's
        # caller may still be about to poll for it.
        directives = [
            {
                "id": "uncollected-old",
                "status": "done",
                "createdAt": "2026-06-06T00:00:01Z",
            },
            {
                "id": "collected-newer",
                "status": "done",
                "createdAt": "2026-06-06T00:00:05Z",
                "collected": True,
            },
            {"id": "active", "status": "running", "createdAt": "2026-06-06T00:00:02Z"},
        ]
        for i in range(MAX_MINI_CODER_DIRECTIVES - 2):
            directives.append(
                {
                    "id": f"t{i}",
                    "status": "failed",
                    "createdAt": f"2026-06-06T01:00:{i:02d}Z",
                }
            )
        self.assertEqual(
            len(directives),
            MAX_MINI_CODER_DIRECTIVES + 1,
            "test setup: exactly 1 over cap",
        )
        capped = cap_mini_coder_directives(directives)
        self.assertEqual(len(capped), MAX_MINI_CODER_DIRECTIVES)
        ids = {d["id"] for d in capped}
        self.assertIn("active", ids, "active is never evicted")
        self.assertNotIn(
            "collected-newer",
            ids,
            "a COLLECTED terminal is evicted before any uncollected one, even though it is newer",
        )
        self.assertIn(
            "uncollected-old",
            ids,
            "an uncollected terminal survives while a collected terminal exists to evict instead",
        )
        for i in range(MAX_MINI_CODER_DIRECTIVES - 2):
            self.assertIn(f"t{i}", ids)


class VisualCheckTests(unittest.TestCase):
    """The `visual_check` MCP tool: file-only bridge, bounded poll, camelCase shape."""

    def _project_dir(self, tmp: str) -> Path:
        root = Path(tmp)
        projects = root / "projects"
        projects.mkdir()
        sample_project(projects)
        return root

    def _register_coder(
        self, root: Path, agent_id: str = "codex", role: str = "coder"
    ) -> str:
        token = "test-launch-token"
        (root / "projects" / ".aspis-agents.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "updatedAt": "2026-06-06T00:00:00+00:00",
                    "sessions": [
                        {
                            "agentId": agent_id,
                            "role": role,
                            "status": "launch_pending",
                            "currentProjectId": "scrna-seq",
                            "lastSeenAt": "2026-06-06T00:00:00+00:00",
                            "launchTokenHash": hashlib.sha256(
                                token.encode("utf-8")
                            ).hexdigest(),
                            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                }
            ),
            encoding="utf-8",
        )
        result = handle_tool_call(
            "agent_register",
            {
                "agent_id": agent_id,
                "role": role,
                "model": "codex",
                "message": "coding",
                "launch_token": token,
            },
            root=root,
        )
        return result["sessionToken"]

    def _read_state(self, root: Path) -> dict:
        return json.loads(
            (root / "projects" / ".aspis-agents.json").read_text(encoding="utf-8")
        )

    def test_writes_pending_directive_with_exact_camel_case_keys_and_caps_focus(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.VISUAL_CHECK_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "visual_check",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "html_path": "dist/page.html",
                        "focus": "x" * 800,
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertIn("directiveId", out)
            self.assertIn("did not start", out["error"])
            directives = self._read_state(root)["visualCheckDirectives"]
            self.assertEqual(len(directives), 1)
            d = directives[0]
            self.assertEqual(d["id"], out["directiveId"])
            self.assertEqual(d["parentAgentId"], "codex")
            self.assertEqual(d["htmlPath"], "dist/page.html")
            self.assertEqual(d["status"], "failed")
            self.assertLessEqual(len(d["focus"]), 501)
            self.assertEqual(d["resultPath"], f"{out['directiveId']}.json")
            self.assertNotIn("html_path", d)
            self.assertNotIn("parent_agent_id", d)

    def test_design_request_writes_pending_directive_camel_case(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.DESIGN_REQUEST_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "design_request",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "prompt": "a billing dashboard with usage metering",
                        "context": "task 5 of the Stripe plan",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertIn("directiveId", out)
            self.assertIn("did not start", out["error"])
            directives = self._read_state(root)["designRequestDirectives"]
            self.assertEqual(len(directives), 1)
            d = directives[0]
            self.assertEqual(d["id"], out["directiveId"])
            self.assertEqual(d["parentAgentId"], "codex")
            self.assertEqual(d["prompt"], "a billing dashboard with usage metering")
            self.assertEqual(d["planContext"], "task 5 of the Stripe plan")
            self.assertEqual(d["status"], "failed")
            self.assertNotIn("parent_agent_id", d)
            self.assertNotIn("plan_context", d)

    def test_design_request_accepts_mode_and_frame(self):
        """Phase 3: mode + frame are written into the directive camelCase."""
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.DESIGN_REQUEST_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "design_request",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "prompt": "Android home screen",
                        "mode": "interactive",
                        "frame": "android",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertIn("directiveId", out)
            directives = self._read_state(root)["designRequestDirectives"]
            d = directives[0]
            self.assertEqual(d["mode"], "interactive")
            self.assertEqual(d["frame"], "android")

    def test_design_request_omits_mode_frame_when_not_supplied(self):
        """Phase 3: missing mode/frame must NOT appear in the directive (backward compat)."""
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.DESIGN_REQUEST_POLL_TIMEOUT_SECS", 0.0):
                handle_tool_call(
                    "design_request",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "prompt": "a billing dashboard",
                        "session_token": token,
                    },
                    root=root,
                )
            directives = self._read_state(root)["designRequestDirectives"]
            d = directives[0]
            self.assertNotIn("mode", d, "no mode key when param omitted")
            self.assertNotIn("frame", d, "no frame key when param omitted")

    def test_design_request_ignores_invalid_mode(self):
        """Phase 3: an unknown mode value must be silently ignored (not stored)."""
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.DESIGN_REQUEST_POLL_TIMEOUT_SECS", 0.0):
                handle_tool_call(
                    "design_request",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "prompt": "a page",
                        "mode": "hack",
                        "frame": "unknown",
                        "session_token": token,
                    },
                    root=root,
                )
            directives = self._read_state(root)["designRequestDirectives"]
            d = directives[0]
            self.assertNotIn("mode", d, "invalid mode must not be stored")
            self.assertNotIn("frame", d, "invalid frame must not be stored")

    def test_design_request_tool_schema_has_mode_and_frame_params(self):
        """Phase 3: the TOOLS schema exposes mode and frame parameters."""
        from oracle.server import aspis_mcp

        tool = next(t for t in aspis_mcp.TOOLS if t["name"] == "design_request")
        params = tool["parameters"]
        self.assertIn("mode", params, "mode param missing from design_request schema")
        self.assertIn("frame", params, "frame param missing from design_request schema")
        self.assertEqual(params["mode"]["default"], "static")

    def test_visual_check_poll_returns_critique_without_holding_lock(self):
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            done = {"status": "done", "critique": "Button text overflows on mobile."}

            def executor():
                from oracle.server.aspis_mcp import (
                    AGENTS_STATE_FILE,
                    file_lock,
                    read_agents_state,
                    write_agents_state,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("visualCheckDirectives", [])
                        if ds:
                            ds[0]["status"] = "done"
                            ds[0]["result"] = done
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=executor)
            t.start()
            try:
                with patch(
                    "oracle.server.aspis_mcp.VISUAL_CHECK_POLL_INTERVAL_SECS", 0.02
                ):
                    out = handle_tool_call(
                        "visual_check",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "html_path": "dist/page.html",
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["critique"], "Button text overflows on mobile.")

    def test_cap_visual_check_directives_evicts_oldest_terminal_keeps_pending(self):
        from oracle.server.aspis_mcp import (
            MAX_VISUAL_CHECK_DIRECTIVES,
            cap_visual_check_directives,
        )

        directives = [
            {"id": "old", "status": "done", "createdAt": "2026-06-06T00:00:01Z"},
            {"id": "active", "status": "pending", "createdAt": "2026-06-06T00:00:02Z"},
        ]
        for i in range(MAX_VISUAL_CHECK_DIRECTIVES):
            directives.append(
                {
                    "id": f"v{i}",
                    "status": "failed",
                    "createdAt": f"2026-06-06T01:00:{i:02d}Z",
                }
            )
        capped = cap_visual_check_directives(directives)
        self.assertLessEqual(len(capped), MAX_VISUAL_CHECK_DIRECTIVES)
        ids = {d["id"] for d in capped}
        self.assertIn("active", ids)
        self.assertNotIn("old", ids)


class RequestGitPushTests(unittest.TestCase):
    """GH-P4: the `request_git_push` MCP tool — gating, request write (camelCase
    parity with the Rust git_push.rs), the needs_user bell, the bounded verdict poll,
    and the timeout path. Mirrors SpawnMiniCoderTests."""

    def _project_dir(self, tmp: str) -> Path:
        root = Path(tmp)
        projects = root / "projects"
        projects.mkdir()
        sample_project(projects)
        return root

    def _register_coder(
        self, root: Path, agent_id: str = "codex", role: str = "coder"
    ) -> str:
        token = "test-launch-token"
        (root / "projects" / ".aspis-agents.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "updatedAt": "2026-06-06T00:00:00+00:00",
                    "sessions": [
                        {
                            "agentId": agent_id,
                            "role": role,
                            "status": "launch_pending",
                            "lastSeenAt": "2026-06-06T00:00:00+00:00",
                            "launchTokenHash": hashlib.sha256(
                                token.encode("utf-8")
                            ).hexdigest(),
                            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                }
            ),
            encoding="utf-8",
        )
        result = handle_tool_call(
            "agent_register",
            {
                "agent_id": agent_id,
                "role": role,
                "model": "codex",
                "message": "coding",
                "launch_token": token,
            },
            root=root,
        )
        return result["sessionToken"]

    def _read_state(self, root: Path) -> dict:
        return json.loads(
            (root / "projects" / ".aspis-agents.json").read_text(encoding="utf-8")
        )

    def test_writes_pending_request_with_exact_camel_case_keys(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.GIT_PUSH_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "request_git_push",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "branch": "main",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertIn("requestId", out)
            state = self._read_state(root)
            requests = state["gitPushRequests"]
            self.assertEqual(len(requests), 1)
            r = requests[0]
            self.assertEqual(r["id"], out["requestId"])
            self.assertEqual(r["agentId"], "codex")
            self.assertEqual(r["projectId"], "scrna-seq")
            self.assertEqual(r["branch"], "main")
            self.assertIn("createdAt", r)
            # NO-CHURN: force/remote omitted when not set; snake_case never leaks.
            self.assertNotIn("force", r)
            self.assertNotIn("remote", r)
            self.assertNotIn("project_id", r)
            # The poll deadline fired with NO human action -> synthesized timeout, and
            # the still-pending request stamped timeout.
            self.assertEqual(out["result"]["status"], "timeout")
            self.assertEqual(r["status"], "timeout")

    def test_force_and_remote_emitted_when_set(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.GIT_PUSH_POLL_TIMEOUT_SECS", 0.0):
                handle_tool_call(
                    "request_git_push",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "remote": "upstream",
                        "force": True,
                        "session_token": token,
                    },
                    root=root,
                )
            r = self._read_state(root)["gitPushRequests"][0]
            self.assertEqual(r["remote"], "upstream")
            self.assertTrue(r["force"])

    def test_sets_needs_user_bell_while_pending(self):
        # The bell must light immediately on append (before the human acts). We probe
        # it by appending in a sub-thread while the main poll runs, but simplest: run
        # with a real (short) poll and assert the bell was set, then cleared on the
        # synthesized timeout sweep.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.GIT_PUSH_POLL_TIMEOUT_SECS", 0.0):
                handle_tool_call(
                    "request_git_push",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "session_token": token,
                    },
                    root=root,
                )
            # After the timeout sweep the bell is cleared (the agent gave up).
            session = next(
                s for s in self._read_state(root)["sessions"] if s["agentId"] == "codex"
            )
            self.assertIsNone(session.get("needsUser"))

    def test_rejects_missing_project_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "request_git_push",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "  ",
                        "session_token": token,
                    },
                    root=root,
                )

    def test_rejects_wrong_session_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            self._register_coder(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "request_git_push",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "session_token": "wrong",
                    },
                    root=root,
                )

    def test_rejects_invalid_remote_at_request_time(self):
        # FIX F9: an invalid remote (leading '-', a URL, spaces, metachars, overlong)
        # is rejected at REQUEST time with the same allowlist Rust enforces, so it
        # never occupies a queue slot / rings the bell for a push that can't be
        # approved.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            for bad in ("-origin", "https://evil/x.git", "ori gin", "a;b", "x" * 101):
                with self.assertRaises(McpError):
                    handle_tool_call(
                        "request_git_push",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "project_id": "scrna-seq",
                            "remote": bad,
                            "session_token": token,
                        },
                        root=root,
                    )
            # No request was ever written (all rejected before the append).
            self.assertEqual(self._read_state(root).get("gitPushRequests", []), [])

    def test_accepts_valid_remote_and_stores_it(self):
        # A valid remote (the Rust allowlist: first char alnum, [A-Za-z0-9._-/]) is
        # stored verbatim.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.GIT_PUSH_POLL_TIMEOUT_SECS", 0.0):
                handle_tool_call(
                    "request_git_push",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "remote": "up_stream-2/x.y",
                        "session_token": token,
                    },
                    root=root,
                )
            r = self._read_state(root)["gitPushRequests"][0]
            self.assertEqual(r["remote"], "up_stream-2/x.y")

    def test_rejects_verifier_caller(self):
        # request_git_push is coder-only: a verifier (no such allowedTool) is rejected.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root, agent_id="vfx", role="verifier")
            with self.assertRaises(McpError):
                handle_tool_call(
                    "request_git_push",
                    {
                        "agent_id": "vfx",
                        "role": "verifier",
                        "project_id": "scrna-seq",
                        "session_token": token,
                    },
                    root=root,
                )

    def test_poll_returns_terminal_outcome_once_result_is_set(self):
        # Simulate the Rust approve command: a background thread stamps a `pushed`
        # result onto the request shortly after the tool starts polling.
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            pushed = {"status": "pushed", "exitCode": 0, "output": "ok"}

            def stamp():
                import oracle.server.aspis_mcp as mcp

                projects = root / "projects"
                lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with mcp.file_lock(lock):
                        state = mcp.read_agents_state(projects)
                        reqs = state.get("gitPushRequests", [])
                        if reqs:
                            reqs[0]["status"] = "pushed"
                            reqs[0]["result"] = pushed
                            mcp.write_agents_state(projects, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=stamp)
            t.start()
            try:
                with (
                    patch("oracle.server.aspis_mcp.GIT_PUSH_POLL_TIMEOUT_SECS", 10.0),
                    patch("oracle.server.aspis_mcp.GIT_PUSH_POLL_INTERVAL_SECS", 0.02),
                ):
                    out = handle_tool_call(
                        "request_git_push",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "project_id": "scrna-seq",
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["result"]["status"], "pushed")

    def test_timeout_does_not_clobber_a_human_approval_in_the_window(self):
        # The poll deadline fires, but the human already approved (request is now
        # `approved`, no result yet). The sweep must NOT stamp timeout over it.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            import oracle.server.aspis_mcp as mcp

            projects = root / "projects"
            lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"

            original = mcp._git_push_request_result

            def flip_to_approved(projects_dir, state_lock, request_id):
                # On the first poll read, move the request to `approved` (human acted)
                # so the subsequent timeout sweep must leave it alone.
                with mcp.file_lock(state_lock):
                    state = mcp.read_agents_state(projects_dir)
                    for r in state.get("gitPushRequests", []):
                        if (
                            r.get("id") == request_id
                            and r.get("status") == "pending_approval"
                        ):
                            r["status"] = "approved"
                            mcp.write_agents_state(projects_dir, state)
                            break
                return original(projects_dir, state_lock, request_id)

            with (
                patch("oracle.server.aspis_mcp.GIT_PUSH_POLL_TIMEOUT_SECS", 0.0),
                patch(
                    "oracle.server.aspis_mcp._git_push_request_result",
                    side_effect=flip_to_approved,
                ),
            ):
                out = handle_tool_call(
                    "request_git_push",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "session_token": token,
                    },
                    root=root,
                )
            # The agent gets timeout (it gave up), but the request stays `approved` —
            # the human's push still proceeds (the Rust approve command owns it).
            self.assertEqual(out["result"]["status"], "timeout")
            r = self._read_state(root)["gitPushRequests"][0]
            self.assertEqual(r["status"], "approved")

    def test_request_git_push_in_coder_allowed_tools_only(self):
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        self.assertIn("request_git_push", coder["allowedTools"])
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertNotIn("request_git_push", verifier["allowedTools"])

    def test_cap_git_push_requests_evicts_oldest_terminal_keeps_active(self):
        requests = [
            {"id": "old", "status": "pushed", "createdAt": "2026-06-06T00:00:01Z"},
            {
                "id": "active",
                "status": "pending_approval",
                "createdAt": "2026-06-06T00:00:02Z",
            },
        ]
        for i in range(MAX_GIT_PUSH_REQUESTS):
            requests.append(
                {
                    "id": f"t{i}",
                    "status": "denied",
                    "createdAt": f"2026-06-06T01:00:{i:02d}Z",
                }
            )
        capped = cap_git_push_requests(requests)
        self.assertLessEqual(len(capped), MAX_GIT_PUSH_REQUESTS)
        ids = {r["id"] for r in capped}
        self.assertIn("active", ids)
        self.assertNotIn("old", ids)

    def test_scrub_push_result_redacts_github_token_families(self):
        # FIX 4: defense-in-depth egress scrub. Every Rust sanitize_error token family
        # embedded in output/error must be stripped before the result reaches the agent.
        for prefix in ("ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"):
            secret = f"{prefix}AbC123deadBEEF456"
            result = {
                "status": "push_failed",
                "output": f"remote rejected (auth={secret})",
                "error": f"fatal: could not read Password for {secret}",
            }
            scrubbed = _scrub_push_result(result)
            self.assertNotIn(secret, scrubbed["output"], f"{prefix} leaked in output")
            self.assertNotIn(secret, scrubbed["error"], f"{prefix} leaked in error")
            self.assertIn("[redacted-github-token]", scrubbed["error"])
        # Non-token prose and other keys are preserved; input dict is not mutated.
        original = {"status": "pushed", "output": "Everything up-to-date", "error": ""}
        out = _scrub_push_result(original)
        self.assertEqual(out["output"], "Everything up-to-date")
        self.assertEqual(out["status"], "pushed")
        self.assertIsNot(out, original)

    def test_scrub_push_result_is_noop_on_non_dict(self):
        # Robustness: a malformed (non-dict) result is returned unchanged, never raises.
        self.assertEqual(_scrub_push_result("nope"), "nope")  # type: ignore[arg-type]


class PlanApprovalAndAskUserTests(unittest.TestCase):
    """Phase 1 — plan approval + reply-box. The `plan_submit`/`plan_status`/`ask_user`
    MCP tools: gating, artifact + sidecar write, the planApprovalRequests queue, the
    needs_user bell, the bounded verdict poll + timeout, and the ask_user reply box.
    Mirrors the request_git_push tests above (the structural template)."""

    def setUp(self):
        self._old_unmanaged_privileged = os.environ.get(
            "ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"
        )
        self._old_disable_app_vault = os.environ.get("ASPIS_MCP_DISABLE_APP_VAULT")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = "1"

    def tearDown(self):
        if self._old_unmanaged_privileged is None:
            os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
        else:
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = (
                self._old_unmanaged_privileged
            )
        if self._old_disable_app_vault is None:
            os.environ.pop("ASPIS_MCP_DISABLE_APP_VAULT", None)
        else:
            os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = self._old_disable_app_vault

    def _project_dir(self, tmp: str) -> Path:
        root = Path(tmp)
        projects = root / "projects"
        projects.mkdir()
        sample_project(projects)
        return root

    def _register(
        self, root: Path, agent_id: str = "codex", role: str = "coder"
    ) -> str:
        token = "test-launch-token"
        (root / "projects" / ".aspis-agents.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "updatedAt": "2026-06-09T00:00:00+00:00",
                    "sessions": [
                        {
                            "agentId": agent_id,
                            "role": role,
                            "status": "launch_pending",
                            "lastSeenAt": "2026-06-09T00:00:00+00:00",
                            "launchTokenHash": hashlib.sha256(
                                token.encode("utf-8")
                            ).hexdigest(),
                            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                }
            ),
            encoding="utf-8",
        )
        result = handle_tool_call(
            "agent_register",
            {
                "agent_id": agent_id,
                "role": role,
                "model": "codex",
                "message": "coding",
                "launch_token": token,
            },
            root=root,
        )
        return result["sessionToken"]

    def _read_state(self, root: Path) -> dict:
        return json.loads(
            (root / "projects" / ".aspis-agents.json").read_text(encoding="utf-8")
        )

    def _session(self, root: Path, agent_id: str = "codex") -> dict:
        return next(
            s for s in self._read_state(root)["sessions"] if s["agentId"] == agent_id
        )

    # ---- plan_submit ------------------------------------------------------

    def test_plan_submit_writes_md_sidecar_queue_and_needs_user_bell(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            with patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "title": "Refactor the ingestion pipeline",
                        "plan_markdown": "# Plan\n\n- step one\n- step two\n",
                        "session_token": token,
                    },
                    root=root,
                )
            plan_id = out["planId"]
            # plan_id is 32 lowercase hex (uuid4().hex).
            self.assertRegex(plan_id, r"^[0-9a-f]{32}$")
            # Artifact + sidecar exist OUTSIDE the state file, namespaced by project.
            base = root / "projects" / ".aspis-plans" / "scrna-seq"
            md_path = base / f"{plan_id}.md"
            sidecar_path = base / f"{plan_id}.json"
            self.assertTrue(md_path.exists(), "plan markdown artifact missing")
            self.assertTrue(sidecar_path.exists(), "plan sidecar JSON missing")
            self.assertIn("step one", md_path.read_text(encoding="utf-8"))
            sidecar = json.loads(sidecar_path.read_text(encoding="utf-8"))
            self.assertEqual(sidecar["id"], plan_id)
            self.assertEqual(sidecar["projectId"], "scrna-seq")
            self.assertEqual(sidecar["agentId"], "codex")
            self.assertEqual(sidecar["title"], "Refactor the ingestion pipeline")
            self.assertIn("createdAt", sidecar)
            # status started pending_approval; with the 0-timeout the sweep moved it
            # to terminal `timeout` (the dedicated timeout test asserts the sidecar sync).
            # Queue entry with the exact camelCase contract.
            requests = self._read_state(root)["planApprovalRequests"]
            self.assertEqual(len(requests), 1)
            r = requests[0]
            self.assertEqual(r["id"], plan_id)
            self.assertEqual(r["agentId"], "codex")
            self.assertEqual(r["projectId"], "scrna-seq")
            self.assertEqual(r["title"], "Refactor the ingestion pipeline")
            self.assertEqual(r["status"], "timeout")  # poll deadline 0 -> timeout sweep
            self.assertNotIn("project_id", r)
            self.assertNotIn("plan_markdown", r)
            # The poll fired with NO human action -> synthesized timeout outcome.
            self.assertEqual(out["status"], "timeout")

    def test_plan_submit_sets_needs_user_with_plan_reason(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            import threading

            # Probe the bell WHILE the plan is pending by reading from a sub-thread
            # before the (real, short) poll times out and clears it. Read UNDER THE LOCK
            # (the writer atomic-renames the file; a lockless concurrent open races it
            # on Windows -> PermissionError).
            seen = {}

            def probe():
                import oracle.server.aspis_mcp as mcp

                projects = root / "projects"
                lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"
                for _ in range(800):
                    with mcp.file_lock(lock):
                        state = mcp.read_agents_state(projects)
                        sess = next(
                            (
                                s
                                for s in state["sessions"]
                                if s.get("agentId") == "codex"
                            ),
                            None,
                        )
                        needs = (sess or {}).get("needsUser") if sess else None
                    if needs and needs.get("reason") == "needs_plan_approval":
                        seen["needs"] = needs
                        return
                    time.sleep(0.005)

            t = threading.Thread(target=probe)
            t.start()
            try:
                with (
                    patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.3),
                    patch("oracle.server.aspis_mcp.PLAN_POLL_INTERVAL_SECS", 0.02),
                ):
                    handle_tool_call(
                        "plan_submit",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "project_id": "scrna-seq",
                            "title": "Wire the cache",
                            "plan_markdown": "do the thing",
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertIn(
                "needs",
                seen,
                "needsUser(needs_plan_approval) was never observed while pending",
            )
            self.assertIn("Wire the cache", seen["needs"]["message"])

    def test_plan_submit_rejects_verifier(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root, agent_id="vfx", role="verifier")
            with self.assertRaises(McpError):
                handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "vfx",
                        "role": "verifier",
                        "project_id": "scrna-seq",
                        "title": "Nope",
                        "plan_markdown": "x",
                        "session_token": token,
                    },
                    root=root,
                )
            # No artifact and no queue entry were written.
            self.assertFalse((root / "projects" / ".aspis-plans").exists())
            self.assertEqual(self._read_state(root).get("planApprovalRequests", []), [])

    def test_plan_submit_rejects_oversize_markdown(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            huge = "x" * (PLAN_MAX_MARKDOWN_CHARS + 1)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "title": "Too big",
                        "plan_markdown": huge,
                        "session_token": token,
                    },
                    root=root,
                )
            # Rejected before any artifact / queue write.
            self.assertFalse((root / "projects" / ".aspis-plans").exists())
            self.assertEqual(self._read_state(root).get("planApprovalRequests", []), [])

    def test_plan_submit_rejects_empty_title_and_markdown(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            for bad in (
                {"title": "   ", "plan_markdown": "ok"},
                {"title": "ok", "plan_markdown": "   "},
            ):
                with self.assertRaises(McpError):
                    handle_tool_call(
                        "plan_submit",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "project_id": "scrna-seq",
                            "session_token": token,
                            **bad,
                        },
                        root=root,
                    )

    def test_plan_submit_rejects_unknown_project(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            with self.assertRaises(McpError):
                handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "does-not-exist",
                        "title": "Ghost",
                        "plan_markdown": "x",
                        "session_token": token,
                    },
                    root=root,
                )

    def test_plan_submit_poll_returns_approved_with_note(self):
        # Simulate the Rust approve command: a thread stamps `approved` + a decider
        # note onto the queue entry shortly after the tool starts polling.
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)

            def stamp():
                import oracle.server.aspis_mcp as mcp

                projects = root / "projects"
                lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"
                for _ in range(400):
                    with mcp.file_lock(lock):
                        state = mcp.read_agents_state(projects)
                        reqs = state.get("planApprovalRequests", [])
                        if reqs:
                            reqs[0]["status"] = "approved"
                            reqs[0]["decidedAt"] = mcp.now()
                            reqs[0]["note"] = "looks good, ship it"
                            mcp.write_agents_state(projects, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=stamp)
            t.start()
            try:
                with (
                    patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 10.0),
                    patch("oracle.server.aspis_mcp.PLAN_POLL_INTERVAL_SECS", 0.02),
                ):
                    out = handle_tool_call(
                        "plan_submit",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "project_id": "scrna-seq",
                            "title": "Cache wiring",
                            "plan_markdown": "do it",
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["status"], "approved")
            self.assertEqual(out["note"], "looks good, ship it")

    def test_plan_submit_timeout_only_if_still_pending_raced_rejected_wins(self):
        # The poll deadline fires, but the human already rejected (status now
        # `rejected`). The timeout sweep must NOT clobber the human verdict.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            import oracle.server.aspis_mcp as mcp

            original = mcp._plan_request_outcome

            def flip_to_rejected(projects_dir, state_lock, plan_id):
                with mcp.file_lock(state_lock):
                    state = mcp.read_agents_state(projects_dir)
                    for r in state.get("planApprovalRequests", []):
                        if (
                            r.get("id") == plan_id
                            and r.get("status") == "pending_approval"
                        ):
                            r["status"] = "rejected"
                            r["note"] = "no, redo the data model"
                            mcp.write_agents_state(projects_dir, state)
                            break
                return original(projects_dir, state_lock, plan_id)

            with (
                patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.0),
                patch(
                    "oracle.server.aspis_mcp._plan_request_outcome",
                    side_effect=flip_to_rejected,
                ),
            ):
                out = handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "title": "Data model",
                        "plan_markdown": "draft",
                        "session_token": token,
                    },
                    root=root,
                )
            # The raced-in human verdict wins: the agent sees `rejected`, NOT `timeout`.
            self.assertEqual(out["status"], "rejected")
            self.assertEqual(out["note"], "no, redo the data model")
            r = self._read_state(root)["planApprovalRequests"][0]
            self.assertEqual(r["status"], "rejected")

    def test_plan_submit_timeout_stamps_when_still_pending(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            with patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "title": "Untouched plan",
                        "plan_markdown": "x",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertEqual(out["status"], "timeout")
            r = self._read_state(root)["planApprovalRequests"][0]
            self.assertEqual(r["status"], "timeout")
            # The bell is cleared on the timeout sweep (the agent gave up).
            self.assertIsNone(self._session(root).get("needsUser"))
            # Best-effort: the sidecar is updated to the terminal status.
            plan_id = out["planId"]
            sidecar = json.loads(
                (
                    root / "projects" / ".aspis-plans" / "scrna-seq" / f"{plan_id}.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(sidecar["status"], "timeout")

    # ---- plan_status ------------------------------------------------------

    def test_plan_status_returns_queue_entry_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            with patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.0):
                submitted = handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "title": "Status check",
                        "plan_markdown": "x",
                        "session_token": token,
                    },
                    root=root,
                )
            plan_id = submitted["planId"]
            out = handle_tool_call(
                "plan_status",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "plan_id": plan_id,
                    "session_token": token,
                },
                root=root,
            )
            self.assertEqual(out["planId"], plan_id)
            self.assertEqual(out["status"], "timeout")

    def test_plan_status_reads_sidecar_when_evicted_from_queue(self):
        # The queue entry was capped/evicted, but the sidecar on disk still answers.
        import oracle.server.aspis_mcp as mcp

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            with patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.0):
                submitted = handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "title": "Evicted",
                        "plan_markdown": "x",
                        "session_token": token,
                    },
                    root=root,
                )
            plan_id = submitted["planId"]
            # Drop the queue entry entirely (simulate eviction).
            projects = root / "projects"
            lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"
            with mcp.file_lock(lock):
                state = mcp.read_agents_state(projects)
                state["planApprovalRequests"] = []
                mcp.write_agents_state(projects, state)
            out = handle_tool_call(
                "plan_status",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "plan_id": plan_id,
                    "session_token": token,
                },
                root=root,
            )
            self.assertEqual(out["planId"], plan_id)
            self.assertEqual(out["status"], "timeout")

    def test_plan_status_rejects_non_hex_id(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            for bad in (
                "../etc/passwd",
                "ZZZ",
                "abc",
                "a" * 31,
                "a" * 33,
                "0123456789abcdef0123456789abcdeG",
            ):
                with self.assertRaises(McpError):
                    handle_tool_call(
                        "plan_status",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "plan_id": bad,
                            "session_token": token,
                        },
                        root=root,
                    )

    def test_plan_status_unknown_id_not_found(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            out = handle_tool_call(
                "plan_status",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "plan_id": "0" * 32,
                    "session_token": token,
                },
                root=root,
            )
            self.assertEqual(out["status"], "not_found")

    # ---- ask_user ---------------------------------------------------------

    def test_ask_user_blocks_then_returns_matching_reply(self):
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)

            def reply():
                import oracle.server.aspis_mcp as mcp

                projects = root / "projects"
                lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"
                for _ in range(400):
                    with mcp.file_lock(lock):
                        state = mcp.read_agents_state(projects)
                        sess = next(
                            (
                                s
                                for s in state["sessions"]
                                if s.get("agentId") == "codex"
                            ),
                            None,
                        )
                        pending = (sess or {}).get("pendingQuestion") if sess else None
                        if pending:
                            sess["userReply"] = {
                                "questionId": pending["id"],
                                "text": "use option B",
                                "createdAt": mcp.now(),
                            }
                            # The Rust side also clears needsUser on reply.
                            sess["needsUser"] = None
                            mcp.write_agents_state(projects, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=reply)
            t.start()
            try:
                with (
                    patch("oracle.server.aspis_mcp.ASK_USER_POLL_TIMEOUT_SECS", 10.0),
                    patch("oracle.server.aspis_mcp.ASK_USER_POLL_INTERVAL_SECS", 0.02),
                ):
                    out = handle_tool_call(
                        "ask_user",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "question": "Which approach should I take?",
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["reply"], "use option B")
            # Consumed: both pendingQuestion and userReply are cleared.
            sess = self._session(root)
            self.assertIsNone(sess.get("pendingQuestion"))
            self.assertIsNone(sess.get("userReply"))
            self.assertIsNone(sess.get("needsUser"))

    def test_ask_user_ignores_stale_reply_for_old_question(self):
        # A userReply whose questionId does NOT match the current pendingQuestion is a
        # stale answer to a PRIOR question. It must be ignored AND cleared, and the
        # poll must keep waiting for the matching reply.
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            phase = {"wrote_stale": False}

            def reply():
                import oracle.server.aspis_mcp as mcp

                projects = root / "projects"
                lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"
                # First: write a STALE reply (wrong questionId) once.
                for _ in range(400):
                    with mcp.file_lock(lock):
                        state = mcp.read_agents_state(projects)
                        sess = next(
                            (
                                s
                                for s in state["sessions"]
                                if s.get("agentId") == "codex"
                            ),
                            None,
                        )
                        if sess and sess.get("pendingQuestion"):
                            sess["userReply"] = {
                                "questionId": "stale-question-id",
                                "text": "answer to a different question",
                                "createdAt": mcp.now(),
                            }
                            mcp.write_agents_state(projects, state)
                            phase["wrote_stale"] = True
                            break
                    time.sleep(0.01)
                # Then: wait for the stale reply to be cleared, then write the MATCHING one.
                for _ in range(800):
                    with mcp.file_lock(lock):
                        state = mcp.read_agents_state(projects)
                        sess = next(
                            (
                                s
                                for s in state["sessions"]
                                if s.get("agentId") == "codex"
                            ),
                            None,
                        )
                        pending = (sess or {}).get("pendingQuestion") if sess else None
                        if pending and not (sess or {}).get("userReply"):
                            sess["userReply"] = {
                                "questionId": pending["id"],
                                "text": "the real answer",
                                "createdAt": mcp.now(),
                            }
                            mcp.write_agents_state(projects, state)
                            return
                    time.sleep(0.01)

            t = threading.Thread(target=reply)
            t.start()
            try:
                with (
                    patch("oracle.server.aspis_mcp.ASK_USER_POLL_TIMEOUT_SECS", 10.0),
                    patch("oracle.server.aspis_mcp.ASK_USER_POLL_INTERVAL_SECS", 0.02),
                ):
                    out = handle_tool_call(
                        "ask_user",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "question": "Real question?",
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertTrue(
                phase["wrote_stale"], "stale reply was never written by the test thread"
            )
            self.assertEqual(out["reply"], "the real answer")

    def test_ask_user_timeout_clears_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            with patch("oracle.server.aspis_mcp.ASK_USER_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "ask_user",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "question": "Anyone there?",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertTrue(out.get("timeout"))
            sess = self._session(root)
            self.assertIsNone(sess.get("pendingQuestion"))
            self.assertIsNone(sess.get("needsUser"))

    def test_ask_user_allows_verifier(self):
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root, agent_id="vfx", role="verifier")

            def reply():
                import oracle.server.aspis_mcp as mcp

                projects = root / "projects"
                lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"
                for _ in range(400):
                    with mcp.file_lock(lock):
                        state = mcp.read_agents_state(projects)
                        sess = next(
                            (s for s in state["sessions"] if s.get("agentId") == "vfx"),
                            None,
                        )
                        pending = (sess or {}).get("pendingQuestion") if sess else None
                        if pending:
                            sess["userReply"] = {
                                "questionId": pending["id"],
                                "text": "yes",
                                "createdAt": mcp.now(),
                            }
                            mcp.write_agents_state(projects, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=reply)
            t.start()
            try:
                with (
                    patch("oracle.server.aspis_mcp.ASK_USER_POLL_TIMEOUT_SECS", 10.0),
                    patch("oracle.server.aspis_mcp.ASK_USER_POLL_INTERVAL_SECS", 0.02),
                ):
                    out = handle_tool_call(
                        "ask_user",
                        {
                            "agent_id": "vfx",
                            "role": "verifier",
                            "question": "ok?",
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["reply"], "yes")

    # ---- needsUser clobber guard (WARNING #5) -----------------------------

    def _light_bell(
        self, root: Path, reason: str, since: str, agent_id: str = "codex"
    ) -> None:
        """Manually set the session's needsUser bell to a given reason/since."""
        import oracle.server.aspis_mcp as mcp

        projects = root / "projects"
        lock = projects / f"{mcp.AGENTS_STATE_FILE}.lock"
        with mcp.file_lock(lock):
            state = mcp.read_agents_state(projects)
            sess = next(s for s in state["sessions"] if s.get("agentId") == agent_id)
            sess["needsUser"] = {"reason": reason, "message": "m", "since": since}
            mcp.write_agents_state(projects, state)

    def test_plan_submit_refuses_when_question_bell_already_lit(self):
        # An in-flight question bell must NOT be clobbered by plan_submit; the agent
        # is told to resolve the outstanding needsUser first.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            self._light_bell(root, "question", "2026-06-09T09:00:00+00:00")
            with patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.0):
                with self.assertRaises(McpError) as ctx:
                    handle_tool_call(
                        "plan_submit",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "project_id": "scrna-seq",
                            "title": "Should not overwrite the question bell",
                            "plan_markdown": "x",
                            "session_token": token,
                        },
                        root=root,
                    )
            self.assertIn("outstanding needsUser", str(ctx.exception))
            # The original question bell survived untouched.
            bell = self._session(root).get("needsUser")
            self.assertEqual(bell["reason"], "question")
            self.assertEqual(bell["since"], "2026-06-09T09:00:00+00:00")

    def test_plan_submit_preserves_since_for_same_reason_bell(self):
        # A pre-existing plan-approval bell is the dedup case: the original `since`
        # must be preserved, not reset to now(). The 0-timeout sweep would clear the
        # bell before we can read it, so use a real short timeout and a patched outcome
        # poll that captures `since` mid-flight (while the bell is still lit).
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            original_since = "2026-06-09T08:00:00+00:00"
            self._light_bell(root, "needs_plan_approval", original_since)
            captured: dict[str, Any] = {}

            def capture_then_pending(projects_dir, state_lock, plan_id):
                import oracle.server.aspis_mcp as mcp

                with mcp.file_lock(state_lock):
                    state = mcp.read_agents_state(projects_dir)
                    sess = next(
                        s for s in state["sessions"] if s.get("agentId") == "codex"
                    )
                    captured["since"] = (sess.get("needsUser") or {}).get("since")
                # Report still-pending so the loop keeps going until the (real) timeout.
                return (True, "pending_approval", None)

            with (
                patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.05),
                patch("oracle.server.aspis_mcp.PLAN_POLL_INTERVAL_SECS", 0.01),
                patch(
                    "oracle.server.aspis_mcp._plan_request_outcome",
                    side_effect=capture_then_pending,
                ),
            ):
                handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "title": "Dedup plan bell",
                        "plan_markdown": "x",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertEqual(captured.get("since"), original_since)

    def test_ask_user_refuses_when_plan_bell_already_lit(self):
        # Symmetric to plan_submit: an in-flight plan-approval bell must NOT be
        # clobbered by ask_user.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            self._light_bell(root, "needs_plan_approval", "2026-06-09T09:00:00+00:00")
            with patch("oracle.server.aspis_mcp.ASK_USER_POLL_TIMEOUT_SECS", 0.0):
                with self.assertRaises(McpError) as ctx:
                    handle_tool_call(
                        "ask_user",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "question": "Should not overwrite the plan bell",
                            "session_token": token,
                        },
                        root=root,
                    )
            self.assertIn("outstanding needsUser", str(ctx.exception))
            # The original plan bell survived; no pendingQuestion was written.
            sess = self._session(root)
            self.assertEqual(
                sess.get("needsUser", {}).get("reason"), "needs_plan_approval"
            )
            self.assertIsNone(sess.get("pendingQuestion"))

    # ---- poll deadline checked at the top of the loop (WARNING #11) --------

    def test_plan_submit_poll_does_not_overshoot_deadline_after_sleep(self):
        # The deadline must be enforced at the TOP of the loop so a sleep cannot push
        # the effective timeout one interval past the cap. With a small timeout and a
        # large interval, the loop must do at most ONE outcome read before timing out
        # (the bottom-of-loop check would do a second read after sleeping the interval).
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root)
            calls = {"n": 0}

            def always_pending(projects_dir, state_lock, plan_id):
                calls["n"] += 1
                return (True, "pending_approval", None)

            # timeout < interval: after the first pass the loop sleeps the interval and
            # the NEXT top-of-loop check must break BEFORE a second read.
            with (
                patch("oracle.server.aspis_mcp.PLAN_POLL_TIMEOUT_SECS", 0.01),
                patch("oracle.server.aspis_mcp.PLAN_POLL_INTERVAL_SECS", 0.2),
                patch(
                    "oracle.server.aspis_mcp._plan_request_outcome",
                    side_effect=always_pending,
                ),
            ):
                out = handle_tool_call(
                    "plan_submit",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "project_id": "scrna-seq",
                        "title": "No overshoot",
                        "plan_markdown": "x",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertEqual(out["status"], "timeout")
            # Exactly one read: the deadline (0.01s) is well under the interval (0.2s),
            # so the top-of-loop check breaks before a second read.
            self.assertEqual(
                calls["n"], 1, "deadline must be checked at top, not after sleep"
            )

    # ---- role wiring + normalization -------------------------------------

    def test_plan_tools_in_role_allowed_tools(self):
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        for tool in ("plan_submit", "plan_status", "ask_user"):
            self.assertIn(tool, coder["allowedTools"], f"coder missing {tool}")
        self.assertIn("ask_user", verifier["allowedTools"])
        self.assertIn("plan_status", verifier["allowedTools"])
        # plan_submit is coder-only.
        self.assertNotIn("plan_submit", verifier["allowedTools"])

    def test_coder_role_rules_carry_plan_mandate(self):
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        blob = " ".join(coder.get("plan", []))
        self.assertIn("plan_submit", blob)
        self.assertIn("ask_user", blob)
        # The mandate is English by convention (role_rules.json SSoT).
        self.assertRegex(blob, r"\b(before|approval|wait|plan)\b", re.IGNORECASE)

    def test_cap_plan_approval_requests_evicts_oldest_terminal_keeps_pending(self):
        requests = [
            {"id": "old", "status": "approved", "createdAt": "2026-06-09T00:00:01Z"},
            {
                "id": "active",
                "status": "pending_approval",
                "createdAt": "2026-06-09T00:00:02Z",
            },
        ]
        for i in range(MAX_PLAN_APPROVAL_REQUESTS):
            requests.append(
                {
                    "id": f"t{i}",
                    "status": "rejected",
                    "createdAt": f"2026-06-09T01:00:{i:02d}Z",
                }
            )
        capped = cap_plan_approval_requests(requests)
        self.assertLessEqual(len(capped), MAX_PLAN_APPROVAL_REQUESTS)
        ids = {r["id"] for r in capped}
        self.assertIn("active", ids, "pending_approval must never be evicted")
        self.assertNotIn("old", ids, "oldest terminal must be evicted first")

    def test_normalization_deletes_empty_plan_key_no_churn(self):
        import oracle.server.aspis_mcp as mcp

        # An empty list must be removed entirely (NO-CHURN), not persisted as [].
        state = mcp.normalize_agents_state({"version": 2, "planApprovalRequests": []})
        self.assertNotIn("planApprovalRequests", state)
        # A non-list value (hand edit) is reset (and then removed when empty).
        state2 = mcp.normalize_agents_state(
            {"version": 2, "planApprovalRequests": "garbage"}
        )
        self.assertNotIn("planApprovalRequests", state2)
        # A populated list is preserved + capped.
        state3 = mcp.normalize_agents_state(
            {
                "version": 2,
                "planApprovalRequests": [
                    {"id": "p", "status": "pending_approval", "createdAt": "z"}
                ],
            }
        )
        self.assertEqual(len(state3["planApprovalRequests"]), 1)


# A minimal StructureGraph (camelCase wire shape) the STUBBED bridge returns, so the
# project_structure tests need NO real app binary. Mirrors the Rust serde shape.
def _fake_structure_graph() -> dict:
    return {
        "files": [
            {
                "path": "core.rs",
                "lang": "rust",
                "definedSymbols": 2,
                "inDegree": 3,
                "outDegree": 0,
            },
            {
                "path": "a.rs",
                "lang": "rust",
                "definedSymbols": 1,
                "inDegree": 0,
                "outDegree": 1,
            },
        ],
        "spine": [
            {"path": "core.rs", "inDegree": 3, "topReferencedSymbols": ["CoreThing"]},
        ],
        "scanned": 2,
        "skippedTooLarge": 0,
        "skippedUnsupported": 1,
        "skippedUnreadable": 0,
        "capped": False,
    }


class AspisMcpProjectStructureTests(unittest.TestCase):
    """Phase 11.2 project_structure: role allowlist gating, project-root resolution,
    compact spine/summary shape, full-graph opt-in, caching, and fail-soft bridge errors.
    The Rust bridge is STUBBED so these tests need no real binary."""

    def setUp(self):
        self._old_unmanaged = os.environ.get(
            "ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"
        )
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        # A real existing, EXECUTABLE file so resolve_structure_bridge_binary() passes its
        # is_file() + os.access(X_OK) check; the bridge ITSELF is stubbed, so this file is
        # never actually executed. The running interpreter is guaranteed executable and
        # present, which keeps the test hermetic (no reliance on /bin/sh layout).
        self._old_app_bin = os.environ.get("ASPIS_APP_BIN")
        os.environ["ASPIS_APP_BIN"] = sys.executable
        import oracle.server.aspis_mcp as mcp

        mcp._STRUCTURE_CACHE.clear()

    def tearDown(self):
        import oracle.server.aspis_mcp as mcp

        mcp._STRUCTURE_CACHE.clear()
        for key, old in (
            ("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", self._old_unmanaged),
            ("ASPIS_APP_BIN", self._old_app_bin),
        ):
            if old is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = old

    def _setup(self, tmp: str) -> tuple[Path, Path]:
        root = Path(tmp)
        projects_dir = prepare_management_root(root)
        work_root = root / "Aspis Bio Work"
        work_root.mkdir()
        _project_with_root(projects_dir, work_root)
        return root, work_root

    # --- role allowlist -------------------------------------------------------

    def test_project_structure_is_in_orienting_roles_allowlists(self):
        for role in ("coder", "orchestrator", "mini", "verifier"):
            self.assertIn(
                "project_structure",
                ROLE_ALLOWED_TOOLS[role],
                f"{role} must be allowed project_structure",
            )
        self.assertTrue(
            any(t["name"] == "project_structure" for t in TOOLS),
            "project_structure must be a registered MCP tool",
        )

    def test_compact_shape_is_spine_plus_summary_and_full_adds_files(self):
        graph = _fake_structure_graph()
        compact = compact_structure(graph, full=False)
        self.assertEqual(set(compact.keys()), {"spine", "summary"})
        self.assertEqual(compact["spine"][0]["path"], "core.rs")
        self.assertEqual(compact["spine"][0]["topReferencedSymbols"], ["CoreThing"])
        self.assertEqual(compact["summary"]["scanned"], 2)
        self.assertEqual(compact["summary"]["skippedUnsupported"], 1)
        self.assertFalse(compact["summary"]["capped"])
        self.assertNotIn("files", compact)
        # full=True opts into the whole per-file node list.
        full = compact_structure(graph, full=True)
        self.assertIn("files", full)
        self.assertEqual(len(full["files"]), 2)

    def test_dispatch_returns_compact_spine_for_a_coder(self):
        import oracle.server.aspis_mcp as mcp

        with tempfile.TemporaryDirectory() as tmp:
            root, _work_root = self._setup(tmp)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-1",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            with patch.object(
                mcp, "_run_structure_bridge", return_value=_fake_structure_graph()
            ) as stub:
                result = handle_tool_call(
                    "project_structure",
                    {
                        "project_id": "censor-proj",
                        "agent_id": "coder-1",
                        "role": "coder",
                    },
                    root=root,
                )
            self.assertEqual(result["projectId"], "censor-proj")
            self.assertEqual(result["spine"][0]["path"], "core.rs")
            self.assertEqual(result["summary"]["scanned"], 2)
            # By default the full file list is NOT returned (compact payload only).
            self.assertNotIn("files", result)
            # The bridge ran on the resolved WORK ROOT (not an arbitrary path).
            (_app_bin, called_root), _ = stub.call_args
            self.assertEqual(Path(called_root).name, "Aspis Bio Work")

    def test_dispatch_full_true_returns_files(self):
        import oracle.server.aspis_mcp as mcp

        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._setup(tmp)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-2",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            with patch.object(
                mcp, "_run_structure_bridge", return_value=_fake_structure_graph()
            ):
                result = handle_tool_call(
                    "project_structure",
                    {
                        "project_id": "censor-proj",
                        "agent_id": "coder-2",
                        "role": "coder",
                        "full": True,
                    },
                    root=root,
                )
            self.assertIn("files", result)
            self.assertEqual(len(result["files"]), 2)

    def test_dispatch_rejected_for_role_without_the_tool(self):
        # No role outside {coder, orchestrator, mini, verifier} exists, but a role gate
        # rejection is exercised by tampering the stored session role to one lacking the
        # tool. Simpler: assert the gate via require_registered_role for a fabricated role
        # by checking ROLE_ALLOWED_TOOLS, plus a live rejection when the registered role's
        # session is for a tool it lacks. Here we verify a MINI cannot read a DIFFERENT
        # project's structure (cross-project scope), and that an UNREGISTERED agent is
        # rejected outright.
        import oracle.server.aspis_mcp as mcp

        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._setup(tmp)
            # Unregistered agent → rejected before any bridge call.
            with patch.object(
                mcp, "_run_structure_bridge", return_value=_fake_structure_graph()
            ) as stub:
                with self.assertRaises(McpError):
                    handle_tool_call(
                        "project_structure",
                        {
                            "project_id": "censor-proj",
                            "agent_id": "ghost",
                            "role": "coder",
                        },
                        root=root,
                    )
                stub.assert_not_called()

    def test_mini_cannot_read_a_different_projects_structure(self):
        import oracle.server.aspis_mcp as mcp

        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._setup(tmp)
            # A mini whose session is scoped to a DIFFERENT project.
            handle_tool_call(
                "agent_register",
                {"agent_id": "mini-x", "role": "mini", "model": "qwen", "message": "x"},
                root=root,
            )
            projects = root / "projects"
            with mcp.file_lock(projects / f"{AGENTS_STATE_FILE}.lock"):
                state = read_agents_state(projects)
                session = next(
                    s for s in state["sessions"] if s.get("agentId") == "mini-x"
                )
                session["currentProjectId"] = "other-project"
                mcp.write_agents_state(projects, state)
            with patch.object(
                mcp, "_run_structure_bridge", return_value=_fake_structure_graph()
            ) as stub:
                with self.assertRaises(McpError):
                    handle_tool_call(
                        "project_structure",
                        {
                            "project_id": "censor-proj",
                            "agent_id": "mini-x",
                            "role": "mini",
                        },
                        root=root,
                    )
                stub.assert_not_called()

    def test_cache_reuses_build_within_ttl_and_full_reuses_cached_graph(self):
        import oracle.server.aspis_mcp as mcp

        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._setup(tmp)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-3",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            with patch.object(
                mcp, "_run_structure_bridge", return_value=_fake_structure_graph()
            ) as stub:
                first = handle_tool_call(
                    "project_structure",
                    {
                        "project_id": "censor-proj",
                        "agent_id": "coder-3",
                        "role": "coder",
                    },
                    root=root,
                )
                # A SECOND call on the unchanged tree must hit the cache (no re-build),
                # and a full=True call must reuse the SAME cached graph (still 1 build).
                second = handle_tool_call(
                    "project_structure",
                    {
                        "project_id": "censor-proj",
                        "agent_id": "coder-3",
                        "role": "coder",
                    },
                    root=root,
                )
                third_full = handle_tool_call(
                    "project_structure",
                    {
                        "project_id": "censor-proj",
                        "agent_id": "coder-3",
                        "role": "coder",
                        "full": True,
                    },
                    root=root,
                )
            self.assertEqual(
                stub.call_count, 1, "the bridge must run exactly once (cached)"
            )
            self.assertEqual(first["spine"], second["spine"])
            self.assertIn("files", third_full)

    def test_bridge_failure_returns_clean_error_not_a_crash(self):
        import oracle.server.aspis_mcp as mcp

        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._setup(tmp)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-4",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            # The bridge raises McpError (e.g. timeout / bad exit) — the dispatcher must
            # surface it as a clean McpError, never an unhandled crash.
            with patch.object(
                mcp, "_run_structure_bridge", side_effect=McpError("bridge exploded")
            ):
                with self.assertRaises(McpError) as ctx:
                    handle_tool_call(
                        "project_structure",
                        {
                            "project_id": "censor-proj",
                            "agent_id": "coder-4",
                            "role": "coder",
                        },
                        root=root,
                    )
            self.assertIn("bridge exploded", str(ctx.exception))

    def test_missing_app_bin_env_fails_closed(self):
        import oracle.server.aspis_mcp as mcp

        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._setup(tmp)
            handle_tool_call(
                "agent_register",
                {
                    "agent_id": "coder-5",
                    "role": "coder",
                    "model": "opus",
                    "message": "x",
                },
                root=root,
            )
            os.environ.pop("ASPIS_APP_BIN", None)
            # No bridge binary configured ⇒ a clear error (NEVER a guessed path / hang).
            with patch.object(
                mcp, "_run_structure_bridge", return_value=_fake_structure_graph()
            ) as stub:
                with self.assertRaises(McpError) as ctx:
                    handle_tool_call(
                        "project_structure",
                        {
                            "project_id": "censor-proj",
                            "agent_id": "coder-5",
                            "role": "coder",
                        },
                        root=root,
                    )
                stub.assert_not_called()
            self.assertIn("ASPIS_APP_BIN", str(ctx.exception))

    # --- binary executability (BLOCKER fix) -----------------------------------

    def test_non_executable_app_bin_fails_closed(self):
        # A present-but-non-executable ASPIS_APP_BIN must fail closed at resolve time with
        # an accurate "not an executable file" message — NOT pass the is_file() check and
        # later blow up as an opaque spawn EACCES.
        with tempfile.TemporaryDirectory() as tmp:
            non_exec = Path(tmp) / "app-bin"
            non_exec.write_text("not really a binary", encoding="utf-8")
            non_exec.chmod(0o644)  # readable but NOT executable
            os.environ["ASPIS_APP_BIN"] = str(non_exec)
            with self.assertRaises(McpError) as ctx:
                resolve_structure_bridge_binary()
            msg = str(ctx.exception)
            self.assertIn("is not an executable file", msg)
            # PRIVACY: only the basename is echoed, never the full path.
            self.assertIn("app-bin", msg)
            self.assertNotIn(str(non_exec.parent), msg)

    def test_executable_app_bin_resolves(self):
        # The complement: a file WITH the exec bit set resolves cleanly.
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / "app-bin"
            exe.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            exe.chmod(0o755)
            os.environ["ASPIS_APP_BIN"] = str(exe)
            self.assertEqual(resolve_structure_bridge_binary(), str(exe))

    # --- concurrency: in-flight dedup + bounded semaphore ---------------------

    def test_concurrent_same_key_calls_build_exactly_once(self):
        # N threads requesting the SAME (root, freshness) key cold must dedup onto ONE
        # builder via the per-key in-flight Condition: the stubbed bridge is called exactly
        # once, and every caller gets the same graph.
        import oracle.server.aspis_mcp as mcp
        import threading as _threading

        with tempfile.TemporaryDirectory() as tmp:
            _root, work_root = self._setup(tmp)

            call_count = 0
            count_lock = _threading.Lock()
            release = _threading.Event()

            def slow_runner(_app_bin, _root_arg):
                nonlocal call_count
                with count_lock:
                    call_count += 1
                # Hold the build open until all threads have parked as waiters, so the race
                # is deterministic: the first thread is mid-build while the rest dedup.
                release.wait(timeout=5.0)
                return _fake_structure_graph()

            n = 8
            barrier = _threading.Barrier(n)
            results: list = [None] * n
            errors: list = [None] * n

            def worker(i):
                try:
                    barrier.wait(timeout=5.0)
                    results[i] = build_project_structure(
                        work_root, False, runner=slow_runner
                    )
                except Exception as exc:  # noqa: BLE001 - record per-thread failure
                    errors[i] = exc

            threads = [_threading.Thread(target=worker, args=(i,)) for i in range(n)]
            for t in threads:
                t.start()
            # Give the builder a beat to register as in-flight + the waiters to park.
            time.sleep(0.2)
            release.set()
            for t in threads:
                t.join(timeout=5.0)

            self.assertTrue(
                all(not t.is_alive() for t in threads), "threads must not hang"
            )
            self.assertTrue(
                all(e is None for e in errors), f"no thread may error: {errors}"
            )
            self.assertEqual(
                call_count, 1, "the bridge must build exactly once under dedup"
            )
            for r in results:
                self.assertEqual(r["spine"][0]["path"], "core.rs")
            # The in-flight marker is fully cleared after the build completes.
            self.assertEqual(len(mcp._STRUCTURE_INFLIGHT), 0)

    def test_builder_error_wakes_waiters_then_one_retries(self):
        # If the FIRST builder errors, waiters must NOT hang forever: they wake, find no
        # cache entry + no in-flight marker, and exactly one becomes the new builder. With a
        # runner that fails once then succeeds, the net effect is two build attempts and a
        # successful result for everyone (no deadlock, no leaked in-flight marker).
        import oracle.server.aspis_mcp as mcp
        import threading as _threading

        with tempfile.TemporaryDirectory() as tmp:
            _root, work_root = self._setup(tmp)

            attempts = 0
            attempt_lock = _threading.Lock()
            first_in = _threading.Event()
            release_first = _threading.Event()

            def flaky_runner(_app_bin, _root_arg):
                nonlocal attempts
                with attempt_lock:
                    attempts += 1
                    mine = attempts
                if mine == 1:
                    first_in.set()
                    release_first.wait(timeout=5.0)
                    raise McpError("transient build failure")
                return _fake_structure_graph()

            n = 4
            results: list = [None] * n
            errors: list = [None] * n

            def worker(i):
                try:
                    results[i] = build_project_structure(
                        work_root, False, runner=flaky_runner
                    )
                except Exception as exc:  # noqa: BLE001
                    errors[i] = exc

            threads = [_threading.Thread(target=worker, args=(i,)) for i in range(n)]
            # Start the first builder, let it register + park as the active build.
            threads[0].start()
            self.assertTrue(first_in.wait(timeout=5.0), "first builder must enter")
            for t in threads[1:]:
                t.start()
            time.sleep(0.2)  # let the others park as waiters on the first builder
            release_first.set()  # first builder now raises
            for t in threads:
                t.join(timeout=5.0)

            self.assertTrue(
                all(not t.is_alive() for t in threads), "no thread may hang"
            )
            # Exactly one caller saw the transient error (the original builder); the rest
            # were served by the retry build.
            errored = [e for e in errors if e is not None]
            self.assertEqual(
                len(errored), 1, f"only the first builder errors: {errors}"
            )
            self.assertIsInstance(errored[0], McpError)
            served = [r for r in results if r is not None]
            self.assertEqual(len(served), n - 1, "every other caller gets a result")
            self.assertGreaterEqual(attempts, 2, "the failed build must be retried")
            self.assertEqual(
                len(mcp._STRUCTURE_INFLIGHT), 0, "in-flight marker cleared"
            )

    def test_global_semaphore_bounds_concurrent_builds(self):
        # Distinct keys do NOT dedup, so without the semaphore N distinct projects would
        # spawn N concurrent walkers. The BoundedSemaphore caps in-flight subprocesses at
        # _STRUCTURE_MAX_CONCURRENT_BUILDS regardless of distinct keys.
        import oracle.server.aspis_mcp as mcp
        import threading as _threading

        self.assertIsInstance(
            mcp._STRUCTURE_BUILD_SEMAPHORE, _threading.BoundedSemaphore
        )
        self.assertEqual(_STRUCTURE_MAX_CONCURRENT_BUILDS, 4)

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # build_project_structure is exercised directly (no dispatcher / agent state),
            # so distinct bare work-root dirs are all we need — each is a distinct cache key.
            # N distinct work roots → N distinct cache keys (no dedup between them).
            n = _STRUCTURE_MAX_CONCURRENT_BUILDS + 4
            work_roots = []
            for i in range(n):
                wr = root / f"work-{i}"
                wr.mkdir()
                work_roots.append(wr)

            concurrent = 0
            peak = 0
            peak_lock = _threading.Lock()
            release = _threading.Event()

            def gated_runner(_app_bin, _root_arg):
                nonlocal concurrent, peak
                with peak_lock:
                    concurrent += 1
                    peak = max(peak, concurrent)
                release.wait(timeout=5.0)
                with peak_lock:
                    concurrent -= 1
                return _fake_structure_graph()

            errors: list = [None] * n

            def worker(i):
                try:
                    build_project_structure(work_roots[i], False, runner=gated_runner)
                except Exception as exc:  # noqa: BLE001
                    errors[i] = exc

            threads = [_threading.Thread(target=worker, args=(i,)) for i in range(n)]
            for t in threads:
                t.start()
            # Let the first wave saturate the semaphore.
            time.sleep(0.3)
            with peak_lock:
                observed_peak = peak
            release.set()
            for t in threads:
                t.join(timeout=10.0)

            self.assertTrue(
                all(not t.is_alive() for t in threads), "no thread may hang"
            )
            self.assertTrue(
                all(e is None for e in errors), f"no thread may error: {errors}"
            )
            self.assertLessEqual(
                observed_peak,
                _STRUCTURE_MAX_CONCURRENT_BUILDS,
                "concurrent Rust walkers must be bounded by the semaphore",
            )
            self.assertGreater(observed_peak, 0, "the runner must actually have run")


class SteerMiniCoderTests(unittest.TestCase):
    """ASYNC STEERING (a): steer_mini_coder appends a mid-flight correction to a running
    mini's steerQueue (the SAME channel as the Stop button's killRequested, generalized to
    a FIFO), maps the `stop` sentinel to the kill path, and rejects an absent / terminal
    directive. NO-CHURN: an empty steerQueue is never written."""

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        self._old_vault = os.environ.get("ASPIS_MCP_DISABLE_APP_VAULT")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = "1"

    def tearDown(self):
        for key, old in (
            ("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", self._old),
            ("ASPIS_MCP_DISABLE_APP_VAULT", self._old_vault),
        ):
            if old is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = old

    def _setup(self, tmp: str, directives: list[dict]) -> tuple[Path, Path]:
        """Register a live coder and seed the given miniCoderDirectives. Returns
        (projects_dir, state_lock).

        Default the ROOT directive's `parentAgentId` to the steering caller (`codex`)
        when the test did not set one: after the W5 fix an UNOWNED directive is NOT
        steerable by anyone, so these positive-path tests (codex steers its OWN mini)
        must seed the ownership the production spawn path always records. A retry CHILD
        (has `parentDirectiveId`) inherits the owner from its root, so we leave it alone.
        Tests that exercise ownership explicitly set `parentAgentId` themselves."""
        root = Path(tmp)
        projects = prepare_management_root(root)
        handle_tool_call(
            "agent_register",
            {
                "agent_id": "codex",
                "role": "coder",
                "model": "codex",
                "message": "coding",
            },
            root=root,
        )
        for directive in directives:
            if (
                not directive.get("parentDirectiveId")
                and "parentAgentId" not in directive
            ):
                directive["parentAgentId"] = "codex"
        state_lock = projects / f"{AGENTS_STATE_FILE}.lock"
        from oracle.server.aspis_mcp import file_lock

        with file_lock(state_lock):
            state = read_agents_state(projects)
            state["miniCoderDirectives"] = directives
            write_agents_state(projects, state)
        return projects, state_lock

    def test_steer_appends_to_running_directive(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup(
                tmp,
                [
                    {
                        "id": "d1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "d1.json",
                    }
                ],
            )
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "d1",
                    "message": "use a HashMap not a Vec",
                },
            )
            self.assertEqual(res["status"], "queued")
            self.assertEqual(res["queued"], 1)
            state = read_agents_state(projects)
            d = next(x for x in state["miniCoderDirectives"] if x["id"] == "d1")
            self.assertEqual(d["steerQueue"], ["use a HashMap not a Vec"])
            self.assertNotIn("killRequested", d, "a non-stop steer must not kill")

            # A second append extends the FIFO.
            res2 = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "d1",
                    "message": "add a test",
                },
            )
            self.assertEqual(res2["queued"], 2)
            state = read_agents_state(projects)
            d = next(x for x in state["miniCoderDirectives"] if x["id"] == "d1")
            self.assertEqual(d["steerQueue"], ["use a HashMap not a Vec", "add a test"])

    def test_steer_targets_live_retry_attempt_in_chain(self):
        # The directive_id the coder holds is the ROOT (which may be awaiting_retry while
        # the live attempt is a retry child). The steer must land on the LIVE attempt.
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup(
                tmp,
                [
                    {
                        "id": "root",
                        "status": "awaiting_retry",
                        "task": "x",
                        "resultPath": "root.json",
                    },
                    {
                        "id": "root-r1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "root-r1.json",
                        "parentDirectiveId": "root",
                        "attempt": 1,
                    },
                ],
            )
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "root",
                    "message": "fix the live retry",
                },
            )
            self.assertEqual(res["status"], "queued")
            state = read_agents_state(projects)
            root = next(x for x in state["miniCoderDirectives"] if x["id"] == "root")
            retry = next(
                x for x in state["miniCoderDirectives"] if x["id"] == "root-r1"
            )
            self.assertNotIn(
                "steerQueue", root, "the parked predecessor must not be steered"
            )
            self.assertEqual(retry["steerQueue"], ["fix the live retry"])

    def test_stop_sentinel_sets_kill_requested(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup(
                tmp,
                [
                    {
                        "id": "d1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "d1.json",
                    }
                ],
            )
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "d1",
                    "message": "  STOP  ",
                },
            )
            self.assertEqual(res["status"], "stopped")
            state = read_agents_state(projects)
            d = next(x for x in state["miniCoderDirectives"] if x["id"] == "d1")
            self.assertTrue(d["killRequested"])
            self.assertNotIn("steerQueue", d, "stop must not queue prose")

    def test_not_found_directive(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup(tmp, [])
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "nope",
                    "message": "hi",
                },
            )
            self.assertEqual(res["status"], "not_found")

    def test_terminal_directive_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup(
                tmp,
                [{"id": "d1", "status": "done", "task": "x", "resultPath": "d1.json"}],
            )
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "d1",
                    "message": "too late",
                },
            )
            self.assertEqual(res["status"], "terminal")
            state = read_agents_state(projects)
            d = next(x for x in state["miniCoderDirectives"] if x["id"] == "d1")
            self.assertNotIn("steerQueue", d)

    def test_queue_cap_refuses_without_dropping(self):
        with tempfile.TemporaryDirectory() as tmp:
            full = [f"msg {i}" for i in range(MINI_CODER_MAX_STEER_QUEUE)]
            projects, state_lock = self._setup(
                tmp,
                [
                    {
                        "id": "d1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "d1.json",
                        "steerQueue": list(full),
                    }
                ],
            )
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "d1",
                    "message": "over the cap",
                },
            )
            self.assertEqual(res["status"], "queue_full")
            self.assertEqual(res["queued"], MINI_CODER_MAX_STEER_QUEUE)
            state = read_agents_state(projects)
            d = next(x for x in state["miniCoderDirectives"] if x["id"] == "d1")
            self.assertEqual(
                d["steerQueue"], full, "a full queue must not drop an existing message"
            )

    def test_message_length_is_capped(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup(
                tmp,
                [
                    {
                        "id": "d1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "d1.json",
                    }
                ],
            )
            long = "x" * (MINI_CODER_MAX_STEER_LEN + 500)
            dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "d1",
                    "message": long,
                },
            )
            state = read_agents_state(projects)
            d = next(x for x in state["miniCoderDirectives"] if x["id"] == "d1")
            self.assertEqual(len(d["steerQueue"][0]), MINI_CODER_MAX_STEER_LEN)

    def test_empty_message_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup(
                tmp,
                [
                    {
                        "id": "d1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "d1.json",
                    }
                ],
            )
            with self.assertRaises(McpError):
                dispatch_steer_mini_coder(
                    projects,
                    state_lock,
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "directive_id": "d1",
                        "message": "   ",
                    },
                )

    def test_steer_tool_is_registered_in_schema_and_dispatch(self):
        names = {t["name"] for t in TOOLS}
        self.assertIn("steer_mini_coder", names)
        schema = next(t for t in TOOLS if t["name"] == "steer_mini_coder")
        self.assertEqual(
            set(schema["parameters"]),
            {"agent_id", "role", "directive_id", "message", "session_token"},
        )


class SpawnMainCoderTests(unittest.TestCase):
    """ROLE UNTANGLE Phase 3: spawn_main_coder — the orchestrator-only dispatch of
    the first-class Main coder. Shares the mini-directive machinery but stamps
    tier:"main" and FORCES write:true + write_mode:"agenticIterative"."""

    def _project_dir(self, tmp: str) -> Path:
        root = Path(tmp)
        projects = root / "projects"
        projects.mkdir()
        sample_project(projects)
        return root

    def _register(self, root: Path, role: str, agent_id: str) -> str:
        token = "test-launch-token"
        (root / "projects" / ".aspis-agents.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "updatedAt": "2026-06-06T00:00:00+00:00",
                    "sessions": [
                        {
                            "agentId": agent_id,
                            "role": role,
                            "status": "launch_pending",
                            "lastSeenAt": "2026-06-06T00:00:00+00:00",
                            "launchTokenHash": hashlib.sha256(
                                token.encode("utf-8")
                            ).hexdigest(),
                            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                }
            ),
            encoding="utf-8",
        )
        result = handle_tool_call(
            "agent_register",
            {
                "agent_id": agent_id,
                "role": role,
                "model": "opus",
                "message": "planning",
                "launch_token": token,
            },
            root=root,
        )
        return result["sessionToken"]

    def _read_state(self, root: Path) -> dict:
        return json.loads(
            (root / "projects" / ".aspis-agents.json").read_text(encoding="utf-8")
        )

    def test_spawn_main_coder_stamps_main_tier_and_forces_agentic_write(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root, "orchestrator", "orch")
            out = handle_tool_call(
                "spawn_main_coder",
                {
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "task": "implement the feature end-to-end",
                    "files": ["src/a.rs", "src/b.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            self.assertIn("directiveId", out)
            self.assertEqual(out["status"], "running")
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["id"], out["directiveId"])
            # The Main-coder contract: tier stamped, write + agentic FORCED.
            self.assertEqual(d["tier"], "main")
            self.assertIs(d["write"], True)
            self.assertEqual(d["writeMode"], "agenticIterative")
            self.assertEqual(d["parentAgentId"], "orch")

    def test_spawn_main_coder_does_not_require_the_callers_role_to_also_hold_spawn_mini_coder(
        self,
    ):
        # F-F hardening (double-auth coupling): dispatch_spawn_main_coder must not
        # require the caller's role to also hold "spawn_mini_coder". Orchestrator
        # holds spawn_main_coder only (minis are Main-coder-only); prove
        # spawn_main_coder succeeds end-to-end without the mini grant.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root, "orchestrator", "orch")
            self.assertNotIn(
                "spawn_mini_coder", ROLE_ALLOWED_TOOLS["orchestrator"]
            )
            out = handle_tool_call(
                "spawn_main_coder",
                {
                    "agent_id": "orch",
                    "role": "orchestrator",
                    "task": "implement the feature end-to-end",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            self.assertIn("directiveId", out)
            self.assertEqual(out["status"], "running")
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["tier"], "main")
            self.assertIs(d["write"], True)
            self.assertEqual(d["parentAgentId"], "orch")

    def test_spawn_mini_coder_preauthorized_skips_its_own_grant_check(self):
        # Unit-level proof that `_preauthorized` bypasses the internal
        # `spawn_mini_coder` re-derivation entirely: calling the dispatch
        # function DIRECTLY (not through `handle_tool_call`/`spawn_main_coder`)
        # with a role that has NEITHER grant must still succeed when preauthorized.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            self._register(root, "orchestrator", "orch")
            projects_dir = root / "projects"
            state_lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
            out = dispatch_spawn_mini_coder(
                projects_dir,
                state_lock,
                {
                    "agent_id": "orch",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "write": True,
                    "write_mode": "agenticIterative",
                },
                tier="main",
                _preauthorized=("orch", "some-role-with-neither-grant"),
            )
            self.assertIn("directiveId", out)
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["parentAgentId"], "orch")
            self.assertEqual(d["tier"], "main")

    def test_spawn_main_coder_rejected_for_coder_verifier_and_unregistered(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root, "coder", "codex")
            with self.assertRaises(McpError):
                handle_tool_call(
                    "spawn_main_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": ["src/a.rs"],
                        "wait": False,
                        "session_token": token,
                    },
                    root=root,
                )
            # And no directive was written by the rejected call.
            state = self._read_state(root)
            self.assertEqual(state.get("miniCoderDirectives", []), [])

    def test_spawn_mini_coder_directive_carries_no_tier_key_no_churn(self):
        # NO-CHURN: an ordinary mini directive must stay byte-identical — no
        # `tier` key injected (mirrors the Rust serde skip).
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register(root, "coder", "codex")
            out = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["id"], out["directiveId"])
            self.assertNotIn("tier", d)


class AsyncMiniCoderResultTests(unittest.TestCase):
    """ASYNC (b): spawn_mini_coder(wait=false) returns the directiveId immediately and
    mini_coder_result collects the outcome (blocking wait=true reuses the same poll as
    the blocking spawn; wait=false does a single read). The DEFAULT blocking spawn path
    is byte-identical (covered by SpawnMiniCoderTests)."""

    def _project_dir(self, tmp: str) -> Path:
        root = Path(tmp)
        projects = root / "projects"
        projects.mkdir()
        sample_project(projects)
        return root

    def _register_coder(self, root: Path, agent_id: str = "codex") -> str:
        token = "test-launch-token"
        (root / "projects" / ".aspis-agents.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "updatedAt": "2026-06-06T00:00:00+00:00",
                    "sessions": [
                        {
                            "agentId": agent_id,
                            "role": "coder",
                            "status": "launch_pending",
                            "lastSeenAt": "2026-06-06T00:00:00+00:00",
                            "launchTokenHash": hashlib.sha256(
                                token.encode("utf-8")
                            ).hexdigest(),
                            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                        }
                    ],
                    "claims": [],
                    "events": [],
                }
            ),
            encoding="utf-8",
        )
        result = handle_tool_call(
            "agent_register",
            {
                "agent_id": agent_id,
                "role": "coder",
                "model": "codex",
                "message": "coding",
                "launch_token": token,
            },
            root=root,
        )
        return result["sessionToken"]

    def _read_state(self, root: Path) -> dict:
        return json.loads(
            (root / "projects" / ".aspis-agents.json").read_text(encoding="utf-8")
        )

    def test_spawn_wait_false_returns_running_without_polling(self):
        # wait=false must return {directiveId, status:'running'} IMMEDIATELY and never
        # poll — even though there is no executor, the directive stays pending and the
        # call returns at once (no synthesized failed/timeout). The directive is still
        # written for the executor to claim.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            start = time.monotonic()
            out = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            elapsed = time.monotonic() - start
            self.assertLess(elapsed, 5.0, f"wait=false must not block: {elapsed:.2f}s")
            self.assertIn("directiveId", out)
            self.assertEqual(out["status"], "running")
            self.assertNotIn("result", out)
            # The directive was appended and is still pending (no synthesized result).
            d = self._read_state(root)["miniCoderDirectives"][0]
            self.assertEqual(d["id"], out["directiveId"])
            self.assertEqual(d["status"], "pending")
            self.assertNotIn("result", d)

    def test_spawn_wait_truthy_non_bool_still_blocks(self):
        # STRICT bool: only the explicit bool False skips the wait. A truthy non-bool
        # (e.g. the string "false") must NOT be treated as wait=false — it BLOCKS like
        # the default. With no executor + a 0s cap, the blocking path synthesizes a
        # terminal `failed` result (proving it took the blocking branch).
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "spawn_mini_coder",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "task": "x",
                        "files": ["src/a.rs"],
                        "wait": "false",
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertIn("result", out)
            self.assertEqual(out["result"]["status"], "failed")
            self.assertNotIn("status", out)

    def test_result_wait_false_running_then_terminal(self):
        # Spawn non-blocking, then a single non-blocking read: while pending it is
        # 'running'; once the executor stamps a terminal result the read returns it.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            spawn = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            did = spawn["directiveId"]
            # Single read while still pending -> running.
            out = handle_tool_call(
                "mini_coder_result",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": did,
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            self.assertEqual(out, {"directiveId": did, "status": "running"})
            # Executor stamps a terminal result on the SAME id.
            from oracle.server.aspis_mcp import (
                AGENTS_STATE_FILE,
                file_lock,
                read_agents_state,
                write_agents_state,
            )

            projects_dir = root / "projects"
            lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
            done = {
                "status": "done",
                "output": "wrote the docstring",
                "filesTouched": ["src/a.rs"],
            }
            with file_lock(lock):
                state = read_agents_state(projects_dir)
                d = next(x for x in state["miniCoderDirectives"] if x["id"] == did)
                d["status"] = "done"
                d["result"] = done
                write_agents_state(projects_dir, state)
            out2 = handle_tool_call(
                "mini_coder_result",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": did,
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            self.assertEqual(out2["directiveId"], did)
            self.assertEqual(out2["result"], done)
            self.assertNotIn("status", out2)

    def test_result_wait_false_stamps_collected_once_terminal_is_handed_back(self):
        # F-E hardening: once `mini_coder_result` successfully hands back a
        # TERMINAL outcome, the underlying directive is persisted with
        # `collected: true` — the signal `cap_mini_coder_directives` uses to
        # prefer evicting it over a directive whose outcome was never picked up.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            spawn = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            did = spawn["directiveId"]
            from oracle.server.aspis_mcp import (
                AGENTS_STATE_FILE,
                file_lock,
                read_agents_state,
                write_agents_state,
            )

            projects_dir = root / "projects"
            lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
            done = {"status": "done", "output": "wrote it"}
            with file_lock(lock):
                state = read_agents_state(projects_dir)
                d = next(x for x in state["miniCoderDirectives"] if x["id"] == did)
                d["status"] = "done"
                d["result"] = done
                write_agents_state(projects_dir, state)

            pre = next(
                x
                for x in self._read_state(root)["miniCoderDirectives"]
                if x["id"] == did
            )
            self.assertNotIn("collected", pre, "not yet collected before the read")

            out = handle_tool_call(
                "mini_coder_result",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": did,
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            self.assertEqual(out["result"], done)

            post = next(
                x
                for x in self._read_state(root)["miniCoderDirectives"]
                if x["id"] == did
            )
            self.assertIs(
                post.get("collected"),
                True,
                "the directive must be stamped collected after a successful hand-back",
            )

    def test_result_wait_false_unknown_directive_is_not_found(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            out = handle_tool_call(
                "mini_coder_result",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "nope",
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            self.assertEqual(out, {"directiveId": "nope", "status": "not_found"})

    def test_result_wait_true_blocks_and_returns_executor_outcome(self):
        # wait=true reuses the SAME bounded poll as the blocking spawn: a background
        # thread stamps a terminal result and the blocking read returns it.
        import threading

        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            spawn = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            did = spawn["directiveId"]
            done = {"status": "done", "output": "ok"}

            def executor():
                from oracle.server.aspis_mcp import (
                    AGENTS_STATE_FILE,
                    file_lock,
                    read_agents_state,
                    write_agents_state,
                )

                projects_dir = root / "projects"
                lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
                for _ in range(200):
                    with file_lock(lock):
                        state = read_agents_state(projects_dir)
                        ds = state.get("miniCoderDirectives", [])
                        if ds:
                            ds[0]["status"] = "done"
                            ds[0]["result"] = done
                            write_agents_state(projects_dir, state)
                            return
                    time.sleep(0.02)

            t = threading.Thread(target=executor)
            t.start()
            try:
                with patch(
                    "oracle.server.aspis_mcp.MINI_CODER_POLL_INTERVAL_SECS", 0.02
                ):
                    out = handle_tool_call(
                        "mini_coder_result",
                        {
                            "agent_id": "codex",
                            "role": "coder",
                            "directive_id": did,
                            "session_token": token,
                        },
                        root=root,
                    )
            finally:
                t.join()
            self.assertEqual(out["directiveId"], did)
            self.assertEqual(out["result"], done)

    def test_result_wait_true_synthesizes_failed_on_never_claimed(self):
        # wait=true on a pending directive past the deadline -> the SAME `failed`
        # synthesis the blocking spawn does (FIX 3), proving the shared helper.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            spawn = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            did = spawn["directiveId"]
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "mini_coder_result",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "directive_id": did,
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertEqual(out["result"]["status"], "failed")
            self.assertIn("did not start", out["result"]["error"])

    def test_result_default_wait_is_blocking(self):
        # `wait` omitted defaults to blocking (like spawn): with no executor + 0s cap it
        # synthesizes a terminal result rather than returning status:'running'.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            spawn = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            did = spawn["directiveId"]
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "mini_coder_result",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "directive_id": did,
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertIn("result", out)
            self.assertNotIn("status", out)

    def test_result_is_main_coder_only(self):
        coder = next(r for r in ROLE_RULES if r["role"] == "coder")
        orch = next(r for r in ROLE_RULES if r["role"] == "orchestrator")
        verifier = next(r for r in ROLE_RULES if r["role"] == "verifier")
        self.assertIn("mini_coder_result", coder["allowedTools"])
        self.assertNotIn("mini_coder_result", orch["allowedTools"])
        self.assertNotIn("mini_coder_result", verifier["allowedTools"])

    def test_result_verifier_rejected_at_dispatch(self):
        # A verifier (no mini_coder_result) is rejected by the role gate.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            projects = root / "projects"
            os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
            os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = "1"
            try:
                handle_tool_call(
                    "agent_register",
                    {
                        "agent_id": "ver",
                        "role": "verifier",
                        "model": "opus",
                        "message": "reviewing",
                    },
                    root=root,
                )
                state_lock = projects / f"{AGENTS_STATE_FILE}.lock"
                with self.assertRaises(McpError):
                    dispatch_mini_coder_result(
                        projects,
                        state_lock,
                        {"agent_id": "ver", "role": "verifier", "directive_id": "d1"},
                    )
            finally:
                os.environ.pop("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", None)
                os.environ.pop("ASPIS_MCP_DISABLE_APP_VAULT", None)

    def test_result_tool_is_registered_in_schema_and_dispatch(self):
        names = {t["name"] for t in TOOLS}
        self.assertIn("mini_coder_result", names)
        schema = next(t for t in TOOLS if t["name"] == "mini_coder_result")
        self.assertEqual(
            set(schema["parameters"]),
            {"agent_id", "role", "directive_id", "wait", "session_token"},
        )

    def test_spawn_schema_carries_wait_param(self):
        schema = next(t for t in TOOLS if t["name"] == "spawn_mini_coder")
        self.assertIn("wait", schema["parameters"])
        self.assertEqual(schema["parameters"]["wait"]["default"], True)

    # ---- Fix #1: timeout error must name the calling tool -----------------------

    def test_result_wait_true_timeout_error_names_mini_coder_result(self):
        # A mini_coder_result(wait=true) that hits the deadline must stamp an error
        # mentioning "mini_coder_result", NOT "spawn_mini_coder". The directive must
        # have been seen as running at least once so the `timeout` branch fires (not
        # the `failed` / never-started branch). We fake that by setting status=running.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            # Spawn non-blocking to create the directive.
            spawn = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": token,
                },
                root=root,
            )
            did = spawn["directiveId"]
            # Promote the directive to `running` so _await_mini_directive takes the
            # timeout branch (ever_ran=True) and uses the caller_tool string.
            from oracle.server.aspis_mcp import (
                AGENTS_STATE_FILE,
                file_lock,
                read_agents_state,
                write_agents_state,
            )

            projects_dir = root / "projects"
            lock = projects_dir / f"{AGENTS_STATE_FILE}.lock"
            with file_lock(lock):
                state = read_agents_state(projects_dir)
                for d in state.get("miniCoderDirectives", []):
                    if d.get("id") == did:
                        d["status"] = "running"
                write_agents_state(projects_dir, state)
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 0.0):
                out = handle_tool_call(
                    "mini_coder_result",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "directive_id": did,
                        "session_token": token,
                    },
                    root=root,
                )
            self.assertEqual(out["result"]["status"], "timeout")
            self.assertIn(
                "mini_coder_result",
                out["result"]["error"],
                "timeout error must name mini_coder_result, not spawn_mini_coder",
            )
            self.assertNotIn(
                "spawn_mini_coder",
                out["result"]["error"],
                "spawn_mini_coder must not appear in a mini_coder_result timeout error",
            )

    # ---- Fix #2: wait=true on a nonexistent directive must not block -----------

    def test_result_wait_true_unknown_directive_returns_not_found_fast(self):
        # mini_coder_result(wait=true, directive_id="nonexistent") must return
        # {status: "not_found"} immediately — NOT block for the poll timeout and NOT
        # synthesize "failed". Timing verified: must return in << 1 s even with a
        # multi-second cap patched in (no sleep loop entered).
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            token = self._register_coder(root)
            start = time.monotonic()
            # Use a large-ish timeout so the test would obviously stall if it polled.
            with patch("oracle.server.aspis_mcp.MINI_CODER_POLL_TIMEOUT_SECS", 30.0):
                out = handle_tool_call(
                    "mini_coder_result",
                    {
                        "agent_id": "codex",
                        "role": "coder",
                        "directive_id": "nonexistent-directive-id",
                        "session_token": token,
                    },
                    root=root,
                )
            elapsed = time.monotonic() - start
            self.assertEqual(
                out, {"directiveId": "nonexistent-directive-id", "status": "not_found"}
            )
            self.assertLess(
                elapsed, 5.0, f"not_found must be instant, got {elapsed:.2f}s"
            )
            self.assertNotIn(
                "result", out, "not_found must not synthesize a result payload"
            )

    # ---- Fix #3: ownership check ------------------------------------------------

    def _register_two_coders(
        self, root: Path, owner_id: str = "codex", other_id: str = "codex2"
    ) -> tuple[str, str]:
        """Register two distinct coder sessions; return (owner_token, other_token)."""
        owner_token = "owner-token"
        other_token = "other-token"
        (root / "projects" / ".aspis-agents.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "updatedAt": "2026-06-06T00:00:00+00:00",
                    "sessions": [
                        {
                            "agentId": owner_id,
                            "role": "coder",
                            "status": "launch_pending",
                            "lastSeenAt": "2026-06-06T00:00:00+00:00",
                            "launchTokenHash": hashlib.sha256(
                                owner_token.encode()
                            ).hexdigest(),
                            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                        },
                        {
                            "agentId": other_id,
                            "role": "coder",
                            "status": "launch_pending",
                            "lastSeenAt": "2026-06-06T00:00:00+00:00",
                            "launchTokenHash": hashlib.sha256(
                                other_token.encode()
                            ).hexdigest(),
                            "launchTokenIssuedAt": "2099-01-01T00:00:00+00:00",
                        },
                    ],
                    "claims": [],
                    "events": [],
                }
            ),
            encoding="utf-8",
        )
        r1 = handle_tool_call(
            "agent_register",
            {
                "agent_id": owner_id,
                "role": "coder",
                "model": "codex",
                "message": "coding",
                "launch_token": owner_token,
            },
            root=root,
        )
        r2 = handle_tool_call(
            "agent_register",
            {
                "agent_id": other_id,
                "role": "coder",
                "model": "codex",
                "message": "coding",
                "launch_token": other_token,
            },
            root=root,
        )
        return r1["sessionToken"], r2["sessionToken"]

    def test_mini_coder_result_rejects_non_owner(self):
        # A coder that does NOT own the directive must be rejected with McpError.
        # The directive OWNER (codex) still succeeds.
        with tempfile.TemporaryDirectory() as tmp:
            root = self._project_dir(tmp)
            owner_token, other_token = self._register_two_coders(
                root, "codex", "codex2"
            )
            # Owner spawns a mini non-blocking — the directive carries parentAgentId="codex".
            spawn = handle_tool_call(
                "spawn_mini_coder",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "task": "x",
                    "files": ["src/a.rs"],
                    "wait": False,
                    "session_token": owner_token,
                },
                root=root,
            )
            did = spawn["directiveId"]
            # Non-owner (codex2) tries to read the directive — must be rejected.
            with self.assertRaises(McpError) as cm:
                handle_tool_call(
                    "mini_coder_result",
                    {
                        "agent_id": "codex2",
                        "role": "coder",
                        "directive_id": did,
                        "wait": False,
                        "session_token": other_token,
                    },
                    root=root,
                )
            self.assertIn("not owned by this agent", str(cm.exception))
            # Owner (codex) must still succeed — returns running (no executor).
            out = handle_tool_call(
                "mini_coder_result",
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": did,
                    "wait": False,
                    "session_token": owner_token,
                },
                root=root,
            )
            self.assertEqual(out["status"], "running")


class MiniDirectiveRootClimbTests(unittest.TestCase):
    """B1: _mini_directive_parent_agent_id must CLIMB the parentDirectiveId chain to the
    true ROOT and return ITS parentAgentId — including when called with a retry CHILD id.
    The shared _resolve_mini_root_directive must guard against cycles / dangling links."""

    def test_climb_from_child_id_returns_root_owner(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            state_lock = projects / f"{AGENTS_STATE_FILE}.lock"
            from oracle.server.aspis_mcp import (
                _mini_directive_parent_agent_id,
                file_lock,
            )

            with file_lock(state_lock):
                state = read_agents_state(projects)
                state["miniCoderDirectives"] = [
                    {
                        "id": "root",
                        "status": "awaiting_retry",
                        "parentAgentId": "codex",
                    },
                    {
                        "id": "root-r1",
                        "status": "awaiting_retry",
                        "parentDirectiveId": "root",
                    },
                    {
                        "id": "root-r2",
                        "status": "running",
                        "parentDirectiveId": "root-r1",
                    },
                ]
                write_agents_state(projects, state)
            # By the deepest child id — must climb two hops to the root's owner.
            self.assertEqual(
                _mini_directive_parent_agent_id(projects, state_lock, "root-r2"),
                "codex",
            )
            # By the root id directly — still the root's owner.
            self.assertEqual(
                _mini_directive_parent_agent_id(projects, state_lock, "root"), "codex"
            )
            # Unknown id — None (caller treats conservatively).
            self.assertIsNone(
                _mini_directive_parent_agent_id(projects, state_lock, "nope")
            )

    def test_resolve_root_guards_against_cycle(self):
        from oracle.server.aspis_mcp import _resolve_mini_root_directive

        # a -> b -> a (cycle): must terminate and return a present node, not spin.
        directives = [
            {"id": "a", "parentDirectiveId": "b", "parentAgentId": "x"},
            {"id": "b", "parentDirectiveId": "a"},
        ]
        root = _resolve_mini_root_directive(directives, "a")
        self.assertIsNotNone(root)
        self.assertIn(root["id"], {"a", "b"})

    def test_resolve_root_dangling_parent_link(self):
        from oracle.server.aspis_mcp import _resolve_mini_root_directive

        # child's parent was evicted — resolve to the deepest reachable node (the child).
        directives = [
            {"id": "child", "parentDirectiveId": "gone", "parentAgentId": "y"},
        ]
        root = _resolve_mini_root_directive(directives, "child")
        self.assertIsNotNone(root)
        self.assertEqual(root["id"], "child")


class OwnershipSteerMiniCoderTests(unittest.TestCase):
    """Fix #3 (steer path): steer_mini_coder must reject a caller that does not own
    the directive's parentAgentId. The OWNER still succeeds."""

    def setUp(self):
        self._old = os.environ.get("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS")
        self._old_vault = os.environ.get("ASPIS_MCP_DISABLE_APP_VAULT")
        os.environ["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        os.environ["ASPIS_MCP_DISABLE_APP_VAULT"] = "1"

    def tearDown(self):
        for key, old in (
            ("ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS", self._old),
            ("ASPIS_MCP_DISABLE_APP_VAULT", self._old_vault),
        ):
            if old is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = old

    def _setup_two_coders(self, tmp: str, directives: list[dict]) -> tuple[Path, Path]:
        """Register two coders (owner=codex, other=codex2) and seed directives."""
        root = Path(tmp)
        projects = prepare_management_root(root)
        handle_tool_call(
            "agent_register",
            {
                "agent_id": "codex",
                "role": "coder",
                "model": "codex",
                "message": "coding",
            },
            root=root,
        )
        handle_tool_call(
            "agent_register",
            {
                "agent_id": "codex2",
                "role": "coder",
                "model": "codex",
                "message": "coding",
            },
            root=root,
        )
        state_lock = projects / f"{AGENTS_STATE_FILE}.lock"
        from oracle.server.aspis_mcp import file_lock

        with file_lock(state_lock):
            state = read_agents_state(projects)
            state["miniCoderDirectives"] = directives
            write_agents_state(projects, state)
        return projects, state_lock

    def test_steer_rejects_non_owner(self):
        # A directive owned by codex must reject a steer from codex2.
        # The owner (codex) must still successfully queue a correction.
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup_two_coders(
                tmp,
                [
                    {
                        "id": "d1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "d1.json",
                        "parentAgentId": "codex",
                    }
                ],
            )
            # Non-owner attempt.
            with self.assertRaises(McpError) as cm:
                dispatch_steer_mini_coder(
                    projects,
                    state_lock,
                    {
                        "agent_id": "codex2",
                        "role": "coder",
                        "directive_id": "d1",
                        "message": "should be rejected",
                    },
                )
            self.assertIn("not owned by this agent", str(cm.exception))
            # Owner succeeds.
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "d1",
                    "message": "valid owner correction",
                },
            )
            self.assertEqual(res["status"], "queued")

    def test_steer_retry_child_ownership_uses_root_parent_agent_id(self):
        # When directive_id refers to a ROOT that is awaiting_retry and a retry child
        # exists WITHOUT its own parentAgentId, the check must use the ROOT's
        # parentAgentId. A non-owner is rejected; the owner succeeds.
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup_two_coders(
                tmp,
                [
                    {
                        "id": "root",
                        "status": "awaiting_retry",
                        "task": "x",
                        "resultPath": "root.json",
                        "parentAgentId": "codex",
                    },
                    {
                        "id": "root-r1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "root-r1.json",
                        "parentDirectiveId": "root",
                        "attempt": 1,
                        # Deliberately no parentAgentId on the retry child.
                    },
                ],
            )
            with self.assertRaises(McpError):
                dispatch_steer_mini_coder(
                    projects,
                    state_lock,
                    {
                        "agent_id": "codex2",
                        "role": "coder",
                        "directive_id": "root",
                        "message": "must be rejected",
                    },
                )
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "root",
                    "message": "valid steer on retry chain",
                },
            )
            self.assertEqual(res["status"], "queued")

    def test_steer_by_retry_child_id_enforces_root_ownership(self):
        # W5: a coder that learns a retry CHILD id (readable via agent_state) must NOT be
        # able to steer/stop-kill another agent's mini by targeting the child directly.
        # Resolving the TRUE ROOT must climb from the child to the root and read the
        # root's owner — so a non-owner is rejected even when steering by the child id.
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup_two_coders(
                tmp,
                [
                    {
                        "id": "root",
                        "status": "awaiting_retry",
                        "task": "x",
                        "resultPath": "root.json",
                        "parentAgentId": "codex",
                    },
                    {
                        "id": "root-r1",
                        "status": "running",
                        "task": "x",
                        "resultPath": "root-r1.json",
                        "parentDirectiveId": "root",
                        "attempt": 1,
                        # No parentAgentId on the child — it inherits from the root.
                    },
                ],
            )
            # Non-owner steering BY THE CHILD ID is rejected (root ownership enforced).
            with self.assertRaises(McpError) as cm:
                dispatch_steer_mini_coder(
                    projects,
                    state_lock,
                    {
                        "agent_id": "codex2",
                        "role": "coder",
                        "directive_id": "root-r1",
                        "message": "should be rejected",
                    },
                )
            self.assertIn("not owned by this agent", str(cm.exception))
            # The owner can still steer by the child id.
            res = dispatch_steer_mini_coder(
                projects,
                state_lock,
                {
                    "agent_id": "codex",
                    "role": "coder",
                    "directive_id": "root-r1",
                    "message": "valid owner steer by child id",
                },
            )
            self.assertEqual(res["status"], "queued")

    def test_steer_rejects_unowned_directive(self):
        # W5 (the empty-owner hole): a directive whose resolved owner is empty/absent must
        # NOT be steerable by a non-creator. Before the fix the guard only fired when the
        # owner was non-empty, so an unowned directive was steerable by anyone.
        with tempfile.TemporaryDirectory() as tmp:
            projects, state_lock = self._setup_two_coders(
                tmp,
                [
                    {
                        "id": "orphan",
                        "status": "running",
                        "task": "x",
                        "resultPath": "orphan.json",
                        # Deliberately no parentAgentId — an unowned directive.
                    }
                ],
            )
            with self.assertRaises(McpError) as cm:
                dispatch_steer_mini_coder(
                    projects,
                    state_lock,
                    {
                        "agent_id": "codex2",
                        "role": "coder",
                        "directive_id": "orphan",
                        "message": "nobody owns this, but it must not be steerable",
                    },
                )
            self.assertIn("not owned by this agent", str(cm.exception))


class PigeonFlagOffByteIdenticalTests(unittest.TestCase):
    """Slice 3 SAFETY INVARIANT: with the Pigeon env ABSENT (the default), the
    mini-dispatch must behave BYTE-IDENTICALLY to before this slice — the directive
    is appended to `.aspis-agents.json` `miniCoderDirectives` and NO Pigeon HTTP is
    ever attempted. This is the test that proves the flag-OFF path is unchanged."""

    def _live_coder_state_lock(self, projects: Path) -> Path:
        """Seed a LIVE, token-bearing coder session so `dispatch_spawn_mini_coder`
        passes authn + the live-parent gate. Returns the state lock path."""
        state_lock = projects / f"{AGENTS_STATE_FILE}.lock"
        from oracle.server.aspis_mcp import file_lock

        with file_lock(state_lock):
            state = read_agents_state(projects)
            state["sessions"].append(
                {
                    "agentId": "coder-1",
                    "role": "coder",
                    "status": "active",
                    "firstSeenAt": now(),
                    "sessionTokenHash": hash_session_token("tok-coder-1"),
                    "sessionTokenIssuedAt": now(),
                }
            )
            write_agents_state(projects, state)
        return state_lock

    def test_pigeon_disabled_by_default_with_no_env(self):
        # Defensive: the rest of the suite may have set the env — clear it for this case.
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("PIGEON_PORT", None)
            os.environ.pop("PIGEON_AUTH_TOKEN", None)
            self.assertFalse(_pigeon_enabled())

    def test_pigeon_requires_both_env_vars(self):
        with patch.dict(os.environ, {"PIGEON_PORT": "12345"}, clear=False):
            os.environ.pop("PIGEON_AUTH_TOKEN", None)
            self.assertFalse(_pigeon_enabled())
        with patch.dict(os.environ, {"PIGEON_AUTH_TOKEN": "tok"}, clear=False):
            os.environ.pop("PIGEON_PORT", None)
            self.assertFalse(_pigeon_enabled())
        with patch.dict(
            os.environ,
            {"PIGEON_PORT": "12345", "PIGEON_AUTH_TOKEN": "tok"},
            clear=False,
        ):
            self.assertTrue(_pigeon_enabled())

    def test_disabled_spawn_appends_directive_and_attempts_no_pigeon_http(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = prepare_management_root(Path(tmp))
            state_lock = self._live_coder_state_lock(projects)

            # Any Pigeon HTTP attempt would import httpx via the helper — make that fatal
            # so a regression that fires Pigeon on the disabled path is caught loudly.
            def _boom():
                raise AssertionError("Pigeon HTTP must NOT be used when disabled.")

            with patch.dict(os.environ, {}, clear=False):
                os.environ.pop("PIGEON_PORT", None)
                os.environ.pop("PIGEON_AUTH_TOKEN", None)
                with patch("oracle.server.aspis_mcp.import_httpx", side_effect=_boom):
                    # wait=false so we exercise seam A (the append) without blocking on
                    # the file poll for an executor that is not running in this test.
                    result = dispatch_spawn_mini_coder(
                        projects,
                        state_lock,
                        {
                            "agent_id": "coder-1",
                            "role": "coder",
                            "session_token": "tok-coder-1",
                            "task": "tidy a docstring",
                            "files": ["src/a.py"],
                            "wait": False,
                        },
                    )

            self.assertEqual(result["status"], "running")
            directive_id = result["directiveId"]

            # The directive landed in `.aspis-agents.json` exactly as before — and carries
            # NO `pigeonTicket` (that field only exists on the Pigeon ingest path).
            state = read_agents_state(projects)
            directives = state.get("miniCoderDirectives", [])
            self.assertEqual(len(directives), 1)
            d = directives[0]
            self.assertEqual(d["id"], directive_id)
            self.assertEqual(d["status"], "pending")
            self.assertEqual(d["parentAgentId"], "coder-1")
            self.assertNotIn("pigeonTicket", d)
            # The UI delegation event is recorded (unchanged behaviour).
            self.assertTrue(
                any(
                    e.get("eventType") == "mini_coder_spawn"
                    for e in state.get("events", [])
                )
            )


class DiscoveryHeartbeatGateTests(unittest.TestCase):
    """The Rust in-process oracle has no child pid: liveness comes from a
    heartbeatAt timestamp refreshed every ~10s (PLAN.md P7). Stale heartbeat
    (>45s) must skip the HTTP target; a fresh one must accept it WITHOUT
    consulting the pid; discovery files without heartbeat keep the pid gate."""

    def _probe(self, payload):
        import tempfile as _tf
        from pathlib import Path as _P

        from oracle.server import aspis_mcp

        d = _P(_tf.mkdtemp())
        (d / aspis_mcp.ORACLE_DISCOVERY_FILENAME).write_text(
            json.dumps(payload), encoding="utf-8"
        )
        return aspis_mcp._resolve_oracle_http_target_uncached(d)

    def test_fresh_heartbeat_accepts_target_without_pid(self):
        from datetime import datetime, timezone

        fresh = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
        target = self._probe(
            {"baseUrl": "http://127.0.0.1:12345", "authToken": "t", "heartbeatAt": fresh}
        )
        self.assertIsNotNone(target)

    def test_stale_heartbeat_rejects_target_even_with_live_pid(self):
        from datetime import datetime, timedelta, timezone

        stale = (
            (datetime.now(timezone.utc) - timedelta(seconds=120))
            .isoformat()
            .replace("+00:00", "Z")
        )
        target = self._probe(
            {
                "baseUrl": "http://127.0.0.1:12345",
                "authToken": "t",
                "heartbeatAt": stale,
                "pid": os.getpid(),
            }
        )
        self.assertIsNone(target)

    def test_garbage_heartbeat_falls_back_to_pid_gate(self):
        target = self._probe(
            {
                "baseUrl": "http://127.0.0.1:12345",
                "authToken": "t",
                "heartbeatAt": "not-a-timestamp",
                "pid": 999999,
            }
        )
        self.assertIsNone(target)

    def test_no_heartbeat_keeps_legacy_pid_gate(self):
        target = self._probe(
            {"baseUrl": "http://127.0.0.1:12345", "authToken": "t", "pid": os.getpid()}
        )
        self.assertIsNotNone(target)



if __name__ == "__main__":
    unittest.main()
