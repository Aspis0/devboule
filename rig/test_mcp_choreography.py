#!/usr/bin/env python3
"""
MCP choreography scenario for the headless rig.

Drives oracle/server/aspis_mcp.py over stdio, testing:
  1. Happy-path: register → heartbeat → project_get → claim_task → censor_findings
  2. Negative: claim on a paused project is rejected

Gated by RIG=1 (same as test_smoke).
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Gate: only run when RIG=1
# ---------------------------------------------------------------------------
if os.environ.get("RIG") != "1":
    pytest.skip(
        "RIG=1 required; skipping MCP choreography tests", allow_module_level=True
    )

# ---------------------------------------------------------------------------
# Imports from our own rig modules
# ---------------------------------------------------------------------------
from rig.mcp_client import McpStdioClient, McpError  # noqa: E402
from rig.world import make_projects_dir, forge_agent_launch  # noqa: E402

# Repo root (two levels up from rig/)
REPO_ROOT = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
ACTIVE_PROJECT_ID = "test-proj"
PAUSED_PROJECT_ID = "paused-proj"
AGENT_ID = "rig-test-agent"
AGENT_ROLE = "coder"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _read_agents_state(projects_dir: Path) -> dict:
    """Read the .aspis-agents.json file directly for assertions."""
    path = projects_dir / ".aspis-agents.json"
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return {"sessions": [], "claims": [], "events": []}


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.rig
def test_register_claim_choreography():
    """Happy-path: register → heartbeat → project_get → claim_task → censor_findings.

    Asserts the compact-ack contract (sessionToken present, sensitive fields
    absent, RAW response < 4096 bytes) and the full register→claim flow.
    """
    with tempfile.TemporaryDirectory(prefix="rig-mcp-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        # Forge a launch token for our agent
        token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)

        # REPO_ROOT is the real repo root (for PYTHONPATH / oracle import);
        # projects_dir is the temp test fixture.
        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # ---- Step 1: agent_register ----
            reg_result, reg_raw_len = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": token,
                },
                timeout=15,
            )

            # compact_session_ack shape
            assert "sessionToken" in reg_result, (
                f"register must return sessionToken; keys: {list(reg_result.keys())}"
            )
            session_token = reg_result["sessionToken"]
            assert session_token, "sessionToken must be non-empty"

            # Sensitive fields must NOT leak in compact ack
            session = reg_result.get("session", {})
            for leak_key in (
                "launchTokenHash",
                "launchTokenIssuedAt",
                "sessionTokenHash",
                "launchConsumedAt",
            ):
                assert leak_key not in session, (
                    f"compact ack leaked {leak_key} in session"
                )

            # Fleet summary present
            fleet = reg_result.get("fleet", {})
            assert "sessions" in fleet, "fleet must have sessions count"
            assert "active" in fleet, "fleet must have active count"

            # RAW response size guard (round-5: ack must be < 4 KB, not 110 KB)
            assert reg_raw_len < 4096, (
                f"register ack too large: {reg_raw_len} bytes (expected < 4096)"
            )

            # ---- Step 2: agent_heartbeat ----
            hb_result, hb_raw_len = client.call_tool(
                "agent_heartbeat",
                {
                    "agent_id": AGENT_ID,
                    "session_token": session_token,
                    "status": "active",
                    "message": "rig heartbeat",
                },
                timeout=10,
            )

            # Heartbeat uses compact_session_ack without session_token param,
            # so sessionToken is NOT echoed back (the caller already has it).
            # But the sanitized session and fleet summary must still be present.
            hb_session = hb_result.get("session", {})
            for leak_key in (
                "launchTokenHash",
                "launchTokenIssuedAt",
                "sessionTokenHash",
                "launchConsumedAt",
            ):
                assert leak_key not in hb_session, (
                    f"heartbeat compact ack leaked {leak_key} in session"
                )
            assert hb_raw_len < 4096, (
                f"heartbeat ack too large: {hb_raw_len} bytes (expected < 4096)"
            )

            # ---- Step 3: project_get ----
            proj_result, _ = client.call_tool(
                "project_get",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                },
                timeout=10,
            )
            proj = proj_result.get("project", proj_result)
            # project_get returns public_project which has metadata + state
            meta = proj.get("metadata", proj)
            assert meta.get("id") == ACTIVE_PROJECT_ID, (
                f"expected project id {ACTIVE_PROJECT_ID}, got {meta.get('id')}"
            )
            assert meta.get("status") == "active", (
                f"expected status active, got {meta.get('status')}"
            )

            # ---- Step 4: project_claim_task ----
            claim_result, _ = client.call_tool(
                "project_claim_task",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "task_id": "T1",
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                },
                timeout=10,
            )
            # claim returns public_agents_state; verify the claim was recorded
            claims = claim_result.get("claims", [])
            active_claims = [
                c
                for c in claims
                if c.get("agentId") == AGENT_ID
                and c.get("taskId") == "T1"
                and c.get("projectId") == ACTIVE_PROJECT_ID
            ]
            assert len(active_claims) == 1, (
                f"expected exactly 1 claim for T1, got {len(active_claims)}"
            )
            assert active_claims[0].get("role") == AGENT_ROLE

            # ---- Step 5: censor_findings ----
            censor_result, _ = client.call_tool(
                "censor_findings",
                {
                    "project_id": ACTIVE_PROJECT_ID,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                },
                timeout=10,
            )
            # Must return without error; findings list may be empty
            assert "findings" in censor_result or "projectId" in censor_result, (
                f"unexpected censor_findings shape: {list(censor_result.keys())}"
            )


@pytest.mark.rig
def test_register_rejected_with_wrong_token():
    """Proves token VALIDATION actually runs: a deliberately wrong launch_token
    must be rejected with the McpError from validate_launch_token_for_registration
    (aspis_mcp.py:1711).

    Same fixture shape as test_register_claim_choreography, but with a bogus token.
    """
    with tempfile.TemporaryDirectory(prefix="rig-mcp-wrong-token-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        # Forge a valid launch token, then deliberately use a WRONG one.
        _real_token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)
        wrong_token = _real_token + "-tampered"

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            with pytest.raises(McpError, match="Agent launch token is invalid"):
                client.call_tool(
                    "agent_register",
                    {
                        "agent_id": AGENT_ID,
                        "role": AGENT_ROLE,
                        "model": "rig-model",
                        "launch_token": wrong_token,
                    },
                    timeout=15,
                )


@pytest.mark.rig
def test_claim_rejected_on_paused_project():
    """Register OK, then project_claim_task against a paused project → must fail."""
    with tempfile.TemporaryDirectory(prefix="rig-mcp-paused-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # Register successfully
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

            # Try to claim a task on the paused project → must raise McpError
            with pytest.raises(McpError, match="Cannot claim tasks on paused"):
                client.call_tool(
                    "project_claim_task",
                    {
                        "project_id": PAUSED_PROJECT_ID,
                        "task_id": "T1",
                        "agent_id": AGENT_ID,
                        "role": AGENT_ROLE,
                        "session_token": session_token,
                    },
                    timeout=10,
                )
