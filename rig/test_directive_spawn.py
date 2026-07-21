#!/usr/bin/env python3
"""
Rig tests for the spawn_mini_coder / spawn_main_coder MCP choreography.

Gated by RIG=1 (same as the sibling choreography tests).
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Gate
# ---------------------------------------------------------------------------
if os.environ.get("RIG") != "1":
    pytest.skip(
        "RIG=1 required; skipping spawn directive tests", allow_module_level=True
    )

# ---------------------------------------------------------------------------
# Imports
# ---------------------------------------------------------------------------
from rig.mcp_client import McpStdioClient, McpError  # noqa: E402
from rig.world import make_projects_dir, forge_agent_launch  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parent.parent
PROJECT_ID = "test-proj"
AGENT_ID = "rig-spawn-agent"
# "coder" holds spawn_mini_coder; "orchestrator" holds spawn_main_coder only
# (minis are Main-coder-only — role_rules.json).
AGENT_ROLE = "coder"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _read_agents_state(projects_dir: Path) -> dict:
    path = projects_dir / ".aspis-agents.json"
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return {"sessions": [], "miniCoderDirectives": [], "events": []}


def _assert_directive_row(
    state: dict, directive_id: str, expected_parent: str, expected_files: list
) -> dict:
    """Pin the exact directive row written by spawn_mini_coder / spawn_main_coder."""
    directives = state.get("miniCoderDirectives", [])
    rows = [d for d in directives if str(d.get("id")) == directive_id]
    assert len(rows) == 1, f"expected exactly 1 directive row for {directive_id}; got {len(rows)}"
    row = rows[0]
    assert str(row.get("parentAgentId")) == expected_parent, (
        f"parentAgentId mismatch: {row.get('parentAgentId')}"
    )
    assert str(row.get("status")) == "pending", (
        f"status must be 'pending' (just written); got {row.get('status')}"
    )
    assert str(row.get("resultPath")) == f"{directive_id}.json", (
        f"resultPath must be '{{id}}.json'; got {row.get('resultPath')}"
    )
    assert row.get("files") == expected_files, (
        f"files mismatch: {row.get('files')}"
    )
    return row


def _assert_spawn_event(state: dict, agent_id: str) -> bool:
    events = state.get("events", [])
    return any(
        e.get("eventType") == "mini_coder_spawn" and e.get("agentId") == agent_id
        for e in events
    )


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.rig
def test_spawn_mini_coder_writes_directive_row():
    """wait=false returns {directiveId, status:"running"} immediately; the directive
    row lands in .aspis-agents.json with status "pending"; mini_coder_result returns
    the non-terminal status."""
    with tempfile.TemporaryDirectory(prefix="rig-spawn-mini-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # Register (needs session_token for the register call itself).
            reg_result, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": token,
                },
                timeout=15,
            )
            session_token = reg_result["sessionToken"]

            # spawn_mini_coder(wait=false) -> {directiveId, status:"running"} immediately.
            spawn_result, _ = client.call_tool(
                "spawn_mini_coder",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                    "task": "fix the off-by-one in add()",
                    "files": ["src/lib.rs"],
                    "wait": False,
                },
                timeout=10,
            )
            directive_id = spawn_result.get("directiveId")
            assert directive_id, f"expected directiveId in {spawn_result}"
            # Pin the exact status literal returned by the wait=false path.
            assert spawn_result.get("status") == "running", (
                f"wait=false must return status='running'; got {spawn_result}"
            )

            # On-disk: exactly one directive row, pinned fields.
            state = _read_agents_state(projects_dir)
            row = _assert_directive_row(state, directive_id, AGENT_ID, ["src/lib.rs"])
            # resultPath == f"{id}.json"
            assert row.get("resultPath") == f"{directive_id}.json"

            # mini_coder_spawn event exists.
            assert _assert_spawn_event(state, AGENT_ID), (
                "mini_coder_spawn event must exist"
            )

            # mini_coder_result(wait=false) -> {directiveId, status:"running"} (non-terminal).
            result, _ = client.call_tool(
                "mini_coder_result",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                    "directive_id": directive_id,
                    "wait": False,
                },
                timeout=5,
            )
            assert result.get("directiveId") == directive_id
            assert result.get("status") == "running", (
                f"non-terminal mini_coder_result must return status='running'; got {result}"
            )


@pytest.mark.rig
def test_spawn_main_coder_role_gate_and_forced_fields():
    """spawn_main_coder from the WRONG role -> McpError; from the correct role
    (orchestrator) -> row has tier 'main', write true, writeMode 'agenticIterative'."""
    with tempfile.TemporaryDirectory(prefix="rig-spawn-main-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        # WRONG role (coder) -> must be rejected.
        wrong_token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)
        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            reg_result, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": wrong_token,
                },
                timeout=15,
            )
            session_token = reg_result["sessionToken"]
            with pytest.raises(McpError, match="cannot use spawn_main_coder"):
                client.call_tool(
                    "spawn_main_coder",
                    {
                        "agent_id": AGENT_ID,
                        "role": AGENT_ROLE,
                        "session_token": session_token,
                        "task": "do work",
                        "files": ["src/lib.rs"],
                        "wait": False,
                    },
                    timeout=10,
                )

        # CORRECT role (orchestrator) -> forced fields.
        orch_id = "rig-orchestrator"
        orch_token = forge_agent_launch(projects_dir, orch_id, "orchestrator")
        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            reg_result, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": orch_id,
                    "role": "orchestrator",
                    "model": "rig-model",
                    "launch_token": orch_token,
                },
                timeout=15,
            )
            orch_token2 = reg_result["sessionToken"]

            spawn_result, _ = client.call_tool(
                "spawn_main_coder",
                {
                    "agent_id": orch_id,
                    "role": "orchestrator",
                    "session_token": orch_token2,
                    "task": "substantial multi-file work",
                    "files": ["src/lib.rs"],
                    "wait": False,
                },
                timeout=10,
            )
            directive_id = spawn_result.get("directiveId")
            assert directive_id

            state = _read_agents_state(projects_dir)
            directives = [
                d for d in state.get("miniCoderDirectives", [])
                if str(d.get("id")) == directive_id
            ]
            assert len(directives) == 1
            row = directives[0]
            # Forced fields per dispatch_spawn_main_coder.
            assert row.get("tier") == "main", f"tier must be 'main'; got {row.get('tier')}"
            assert row.get("write") is True, f"write must be True; got {row.get('write')}"
            assert row.get("writeMode") == "agenticIterative", (
                f"writeMode must be 'agenticIterative'; got {row.get('writeMode')}"
            )


@pytest.mark.rig
def test_spawn_file_caps():
    """mini cap 64 (len > 64 rejected); main cap 10 (len > 10 rejected).
    The handler uses `len(files) > CAP` (strict greater-than), so exactly CAP is OK."""
    with tempfile.TemporaryDirectory(prefix="rig-spawn-caps-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)
        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            reg_result, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": token,
                },
                timeout=15,
            )
            session_token = reg_result["sessionToken"]

            # 65 files -> rejected (cap 64, strict >).
            with pytest.raises(McpError, match="at most 64 files"):
                client.call_tool(
                    "spawn_mini_coder",
                    {
                        "agent_id": AGENT_ID,
                        "role": AGENT_ROLE,
                        "session_token": session_token,
                        "task": "bulk",
                        "files": [f"src/file-{i}.rs" for i in range(65)],
                        "wait": False,
                    },
                    timeout=5,
                )

            # 64 files -> accepted (cap boundary).
            result, _ = client.call_tool(
                "spawn_mini_coder",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                    "task": "bulk",
                    "files": [f"src/file-{i}.rs" for i in range(64)],
                    "wait": False,
                },
                timeout=10,
            )
            assert result.get("status") == "running"

            # 11 files for main -> rejected (cap 10).
            orch_id = "rig-orchestrator"
            orch_token = forge_agent_launch(projects_dir, orch_id, "orchestrator")
            reg_result, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": orch_id,
                    "role": "orchestrator",
                    "model": "rig-model",
                    "launch_token": orch_token,
                },
                timeout=15,
            )
            orch_token2 = reg_result["sessionToken"]
            with pytest.raises(McpError, match="at most 10 files"):
                client.call_tool(
                    "spawn_main_coder",
                    {
                        "agent_id": orch_id,
                        "role": "orchestrator",
                        "session_token": orch_token2,
                        "task": "bulk",
                        "files": [f"src/file-{i}.rs" for i in range(11)],
                        "wait": False,
                    },
                    timeout=5,
                )

            # 10 files for main -> accepted (cap boundary).
            result, _ = client.call_tool(
                "spawn_main_coder",
                {
                    "agent_id": orch_id,
                    "role": "orchestrator",
                    "session_token": orch_token2,
                    "task": "bulk",
                    "files": [f"src/file-{i}.rs" for i in range(10)],
                    "wait": False,
                },
                timeout=10,
            )
            assert result.get("status") == "running"


@pytest.mark.rig
def test_spawn_requires_live_session():
    """spawn_mini_coder with a bogus session_token -> rejected; no directive row appended."""
    with tempfile.TemporaryDirectory(prefix="rig-spawn-session-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        # Forge a launch token and register the agent.
        token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)
        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": token,
                },
                timeout=15,
            )

            # Bogus session token -> rejected BEFORE any directive is appended.
            # The rejection comes from require_agent_tool (session token invalid),
            # which runs before the live-session gate in dispatch_spawn_mini_coder.
            with pytest.raises(McpError, match="session token is invalid"):
                client.call_tool(
                    "spawn_mini_coder",
                    {
                        "agent_id": AGENT_ID,
                        "role": AGENT_ROLE,
                        "session_token": "bogus-token-never-valid",
                        "task": "fix bug",
                        "files": ["src/lib.rs"],
                        "wait": False,
                    },
                    timeout=5,
                )

            # No directive row appended.
            state = _read_agents_state(projects_dir)
            directives = state.get("miniCoderDirectives", [])
            assert len(directives) == 0, (
                f"no directive row should be appended for a rejected spawn; got {len(directives)}"
            )
