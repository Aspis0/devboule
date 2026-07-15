#!/usr/bin/env python3
"""
Live websearch rig cell (OPT-IN).

Gated by BOTH RIG=1 AND RIG_LIVE=1. Skips with a clear reason if prerequisites
are missing (EXA_API_KEY not set, or pi-web-access extension not loadable).

Scenario:
  1. Spawn the sidecar with PI_CODING_AGENT_DIR=~/.pi/agent (READ-ONLY — never
     writes there) so the pi-web-access extension loads.
  2. Pass EXA_API_KEY through explicitly (driver's passthrough_env knob).
  3. Mock the LLM to return a tool_call for `web_search {query:"anthropic claude"}`.
  4. The pi-web-access extension executes the REAL Exa API call.
  5. Assert:
       - tool_execution_start/end for web_search
       - the devboule.websearch custom message echoed (shape from sidecar.mjs:
         session.sendMessage({role:"user", content:[{type:"text",
         text: JSON.stringify({type:"devboule.websearch", query, results,
         timestamp})}]) ) — documented below; the test asserts the tool was
         called with the right args by inspecting tool_execution_end.result.
       - response success:true

Timeout: ≤90s total. On any network flake the test fails with diagnostics,
never hangs.
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

import pytest

if os.environ.get("RIG") != "1":
    pytest.skip("RIG=1 required; skipping websearch live tests", allow_module_level=True)
if os.environ.get("RIG_LIVE") != "1":
    pytest.skip(
        "RIG_LIVE=1 required for live websearch cell (real API call to Exa)",
        allow_module_level=True,
    )

from rig.sidecar_driver import SidecarSession  # noqa: E402
from rig.world import build_world_in_temp  # noqa: E402
from rig.mock_llm import MockLLMServer  # noqa: E402

AGENT_ID = "rig-test-websearch"
AGENT_ROLE = "main-coder"
PI_AGENT_DIR = Path.home() / ".pi" / "agent"
WEBSEARCH_QUERY = "anthropic claude"

# The exact custom-message shape the sidecar echoes (sidecar.mjs:491-510):
#   session.sendMessage({
#     role: "user",
#     content: [{
#       type: "text",
#       text: JSON.stringify({
#         type: "devboule.websearch",
#         query: event.args?.query || "",
#         results: event.result?.details || {},
#         timestamp: Date.now(),
#       }),
#     }],
#   })
# The test asserts the tool was called with the right query by inspecting
# the tool_execution_end event's args (which carry the original query).

EXPECTED_CUSTOM_MSG_SHAPE = {
    "type": "devboule.websearch",
    "query": WEBSEARCH_QUERY,
    "results": {},  # filled by the real Exa API
    "timestamp": "<dynamic ISO ms>",
}


def _ensure_project_pi_mcp_config(project_root: Path) -> None:
    """Write <project_root>/.pi/mcp.json with an aspis-management entry so the
    sidecar's isAspisMcpConfigured() returns True."""
    import json

    pi_dir = project_root / ".pi"
    pi_dir.mkdir(parents=True, exist_ok=True)
    mcp_cfg = {
        "mcpServers": {
            "aspis-management": {
                "command": sys.executable,
                "args": [
                    "-m",
                    "oracle.server.aspis_mcp",
                    "--root",
                    str(Path(__file__).resolve().parents[1]),
                    "--projects-dir",
                    str(project_root.parent / "projects"),
                ],
                "transport": "stdio",
            }
        }
    }
    (pi_dir / "mcp.json").write_text(json.dumps(mcp_cfg), encoding="utf-8")


@pytest.mark.rig
def test_websearch_live():
    """Live websearch: mock LLM → tool_call web_search → real Exa API → text done."""
    exa_key = os.environ.get("EXA_API_KEY")
    if not exa_key:
        # Check the extension's zero-config default: web-search.json under
        # PI_AGENT_DIR specifies the provider, but Exa still needs an API key.
        ws_json = PI_AGENT_DIR / "web-search.json"
        if ws_json.exists():
            import json as _json

            try:
                ws_cfg = _json.loads(ws_json.read_text())
                provider = ws_cfg.get("provider", "")
            except Exception:
                provider = ""
        else:
            provider = ""
        pytest.skip(
            f"EXA_API_KEY not set (required for live Exa websearch). "
            f"web-search.json provider={provider!r}. "
            f"Set EXA_API_KEY=<key> and RIG_LIVE=1 to run."
        )

    with build_world_in_temp() as world:
        _ensure_project_pi_mcp_config(world.project_root)

        with MockLLMServer() as mock:
            # First response: tool_call for web_search.
            # Second response: text "done".
            mock.set_responses(
                [
                    {
                        "tool": "web_search",
                        "arguments": {"query": WEBSEARCH_QUERY},
                    },
                    "done",
                ]
            )

            with SidecarSession(
                session_id=AGENT_ID,
                agent_role=AGENT_ROLE,
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=PI_AGENT_DIR,
                # EXA_API_KEY survives the leakage strip via passthrough_env.
                passthrough_env=["EXA_API_KEY"],
            ) as session:
                ready = session.wait_ready(timeout=30.0)
                assert ready.data.get("type") == "ready"

                session.send_prompt("ciao")

                # Collect tool_execution events for web_search.
                tool_start = None
                tool_end = None
                import time

                deadline = time.time() + 75.0
                while time.time() < deadline:
                    with session._cond:
                        for e in session.events:
                            t = e.data.get("type")
                            if t == "tool_execution_start":
                                if e.data.get("toolName") == "web_search":
                                    tool_start = e.data
                            elif t == "tool_execution_end":
                                if e.data.get("toolName") == "web_search":
                                    tool_end = e.data
                    if tool_start is not None and tool_end is not None:
                        break
                    session._cond.wait(timeout=1.0)

                assert tool_start is not None, (
                    f"tool_execution_start for web_search not observed.\n"
                    f"Events:\n{session.dump_state()}"
                )
                assert tool_start.get("args", {}).get("query") == WEBSEARCH_QUERY, (
                    f"web_search called with wrong query: {tool_start.get('args')}"
                )

                assert tool_end is not None, (
                    f"tool_execution_end for web_search not observed.\n"
                    f"Events:\n{session.dump_state()}"
                )
                # The real Exa API returns results; assert results is a dict.
                results = tool_end.get("result", {}).get("details", {})
                assert isinstance(results, dict), (
                    f"expected web_search results to be a dict; got {type(results)}"
                )

                # The devboule.websearch custom message shape (documented from
                # sidecar.mjs:491-510): session.sendMessage with a user message
                # carrying JSON {type:"devboule.websearch", query, results, timestamp}.
                # We assert the query matches by checking tool_execution_end.args.
                # (The sendMessage is internal to the sidecar; the test observes
                # the tool's args/result which mirror the custom message content.)

                # Wait for the response event (success:true).
                response = session.wait_response(timeout=30.0)
                assert response.data.get("success") is True, (
                    f"expected response success:true; got: {response.data}"
                )
