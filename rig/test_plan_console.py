#!/usr/bin/env python3
"""
Plan console rig cell.

Gated by RIG=1 only (no RIG_LIVE needed — the plan tool is a local builtin,
no network). Asserts the devboule.plan FIRST-CLASS EVENT when the `plan` tool
executes.

Investigation result (2026-07-15):
  - The `plan` tool is NOT registered by any extension in the loaded config.
    Checked: pi-web-access (web_search, fetch_content, retrieve_content),
    pi-coding-agent SDK v0.80.3 built-in tools (read, bash, edit, write, ls,
    grep, find, truncate), and all ~/.pi/agent/npm/node_modules packages.
  - sidecar.mjs has a forward-compat guard for plan tool completion:
      if (event.type === "tool_execution_end" && event.toolName === "plan")
    so the emit() path exists even though the tool is absent today.
  - The emit shape (sidecar.mjs, after fix):
      {type:"devboule_plan", plan: event.result?.details || {}, timestamp: Date.now()}
  - The Rust EventMapper handles this via the `devboule_plan` arm in handle_event,
    calling handle_devboule_plan() which produces a ConsoleEntry::Chat with role="plan".

Because the `plan` tool is not present in this SDK version, this test is SKIPPED
with a clear reason. If a future SDK version adds the plan tool, the test
structure below shows exactly what to assert:

  1. Mock LLM → tool_call plan {topic: "..."}
  2. Assert tool_execution_start/end for plan
  3. Assert the devboule_plan FIRST-CLASS EVENT appears in-stream:
     {type:"devboule_plan", plan:{...}, timestamp:...}
  4. Assert response success:true
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

import pytest

if os.environ.get("RIG") != "1":
    pytest.skip("RIG=1 required; skipping plan console tests", allow_module_level=True)

# Investigation evidence: the `plan` tool is NOT registered in any loaded
# extension. Print the reason so reviewers know this is intentional, not a bug.
_PLAN_TOOL_NOT_FOUND_REASON = (
    "The `plan` tool is not registered by any extension in the current SDK "
    "(pi-coding-agent v0.80.3, pi-web-access). "
    "sidecar.mjs:521 has a forward-compat guard for it, but the tool is absent. "
    "To enable: add an extension that calls pi.registerTool({name:'plan', ...}) "
    "and re-run this test. The test structure (below) shows the exact assertions "
    "to use once the tool is available."
)

pytest.skip(_PLAN_TOOL_NOT_FOUND_REASON, allow_module_level=True)


# ---------------------------------------------------------------------------
# The test structure for when the plan tool IS available (kept as reference):
# ---------------------------------------------------------------------------
#
# from rig.sidecar_driver import SidecarSession
# from rig.world import build_world_in_temp
# from rig.mock_llm import MockLLMServer
#
# AGENT_ID = "rig-test-plan"
# AGENT_ROLE = "main-coder"
#
# def _ensure_project_pi_mcp_config(project_root):
#     import json
#     pi_dir = project_root / ".pi"
#     pi_dir.mkdir(parents=True, exist_ok=True)
#     mcp_cfg = {
#         "mcpServers": {
#             "aspis-management": {
#                 "command": sys.executable,
#                 "args": [
#                     "-m", "oracle.server.aspis_mcp",
#                     "--root", str(Path(__file__).resolve().parents[1]),
#                     "--projects-dir", str(project_root.parent / "projects"),
#                 ],
#                 "transport": "stdio",
#             }
#         }
#     }
#     (pi_dir / "mcp.json").write_text(json.dumps(mcp_cfg), encoding="utf-8")
#
# @pytest.mark.rig
# def test_plan_console():
#     """Mock LLM → tool_call plan → assert devboule.plan echo + success:true."""
#     with build_world_in_temp() as world:
#         _ensure_project_pi_mcp_config(world.project_root)
#         with MockLLMServer() as mock:
#             mock.set_responses([
#                 {"tool": "plan", "arguments": {"topic": "refactor add function"}},
#                 "done",
#             ])
#             with SidecarSession(
#                 session_id=AGENT_ID,
#                 agent_role=AGENT_ROLE,
#                 mock_base_url=mock.base_url + "/v1",
#                 project_root=world.project_root,
#                 agent_dir=world.agent_dir,
#             ) as session:
#                 ready = session.wait_ready(timeout=30.0)
#                 assert ready.data.get("type") == "ready"
#
#                 session.send_prompt("ciao")
#
#                 # Collect tool_execution events for plan.
#                 tool_start = None
#                 tool_end = None
#                 import time as _time
#                 deadline = _time.time() + 60.0
#                 with session._cond:
#                     while _time.time() < deadline:
#                         for e in session.events:
#                             t = e.data.get("type")
#                             if t == "tool_execution_start" and e.data.get("toolName") == "plan":
#                                 tool_start = e.data
#                             elif t == "tool_execution_end" and e.data.get("toolName") == "plan":
#                                 tool_end = e.data
#                         if tool_start and tool_end:
#                             break
#                         session._cond.wait(timeout=1.0)
#
#                 assert tool_start is not None, f"plan tool_execution_start not observed: {session.dump_state()}"
#                 assert tool_end is not None, f"plan tool_execution_end not observed: {session.dump_state()}"
#
#                 # Assert devboule.plan custom message echo.
#                 plan_echo = None
#                 with session._cond:
#                     for e in session.events:
#                         if e.data.get("type") == "message_start":
#                             msg = e.data.get("message")
#                             if isinstance(msg, dict) and msg.get("role") == "user":
#                                 content = msg.get("content", [])
#                                 if isinstance(content, list) and content:
#                                     text = content[0].get("text", "") if isinstance(content[0], dict) else ""
#                                     if text.startswith("{"):
#                                         try:
#                                             parsed = json.loads(text)
#                                             if parsed.get("type") == "devboule.plan":
#                                                 plan_echo = parsed
#                                         except (json.JSONDecodeError, KeyError):
#                                             pass
#                 assert plan_echo is not None, f"devboule.plan echo not found: {session.dump_state()}"
#                 assert plan_echo.get("plan"), f"expected non-empty plan payload; got: {plan_echo}"
#
#                 # Assert response success:true.
#                 response = session.wait_response(timeout=30.0)
#                 assert response.data.get("success") is True, f"expected success:true; got: {response.data}"
