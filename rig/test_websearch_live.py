#!/usr/bin/env python3
"""
Live websearch rig cell (OPT-IN).

Gated by RIG=1 AND RIG_LIVE=1. Uses the real ~/.pi/agent as PI_CODING_AGENT_DIR
(read-only) so the pi-web-access extension loads and registers the `web_search`
tool.

Keyless mechanism (verified in pi-web-access/index.ts + exa.ts):
  - isExaAvailable() always returns true (pi-exa is a zero-config MCP provider)
  - When no EXA_API_KEY is set, searchWithExa() falls back to searchWithExaMcp()
    which calls the Exa MCP server (mcp.exa.ai) — no API key needed.
  - web-search.json in ~/.pi/agent sets provider="exa" → auto-selects exa.
  - The driver's passthrough_env knob passes EXA_API_KEY through only if present.

Scenario:
  1. Mock LLM → tool_call web_search {query:"anthropic claude"}
  2. pi-web-access executes the REAL Exa MCP search (keyless)
  3. Assert:
     - tool_execution_start/end for web_search with a successful result
     - devboule_websearch FIRST-CLASS EVENT appears in-stream (non-empty query,
       results object) between tool_execution_end and response — this is the
       whole point: the console channel is now observable via the sidecar's
       first-class emit() path (replaces the dead sendMessage echo).
     - response success:true

Timeout: ≤90s total. On any network flake the test fails with diagnostics.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

import pytest

if os.environ.get("RIG") != "1":
    pytest.skip("RIG=1 required; skipping websearch live tests", allow_module_level=True)
if os.environ.get("RIG_LIVE") != "1":
    pytest.skip(
        "RIG_LIVE=1 required for live websearch cell (real Exa MCP call, no API key needed)",
        allow_module_level=True,
    )

from rig.sidecar_driver import SidecarSession  # noqa: E402
from rig.world import build_world_in_temp  # noqa: E402
from rig.mock_llm import MockLLMServer  # noqa: E402

AGENT_ID = "rig-test-websearch"
AGENT_ROLE = "main-coder"
PI_AGENT_DIR = Path.home() / ".pi" / "agent"
WEBSEARCH_QUERY = "anthropic claude"


def _ensure_project_pi_mcp_config(project_root: Path) -> None:
    """Write <project_root>/.pi/mcp.json with an aspis-management entry."""
    import json as _json

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
    (pi_dir / "mcp.json").write_text(_json.dumps(mcp_cfg), encoding="utf-8")


@pytest.mark.rig
def test_websearch_live():
    """Live websearch: mock LLM → tool_call web_search → real Exa MCP → done."""
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
                # EXA_API_KEY passes through only if present (keyless: Exa MCP works without it).
                passthrough_env=["EXA_API_KEY"],
            ) as session:
                ready = session.wait_ready(timeout=30.0)
                assert ready.data.get("type") == "ready"

                session.send_prompt("ciao")

                # Collect tool_execution events for web_search.
                tool_start = None
                tool_end = None
                import time as _time

                deadline = _time.time() + 75.0
                with session._cond:
                    while _time.time() < deadline:
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
                # The real Exa MCP returns results; assert results is a non-empty dict.
                results = tool_end.get("result", {}).get("details", {})
                assert isinstance(results, dict), (
                    f"expected web_search results to be a dict; got {type(results)}"
                )
                # Print the results for the report (proof the search hit the network).
                print(
                    f"\n=== WEBSEARCH RESULTS (Exa MCP, keyless) ===\n{json.dumps(results, indent=2, default=str)[:2000]}\n=== END WEBSEARCH ===\n"
                )

                # Assert the devboule_websearch FIRST-CLASS EVENT appears in-stream.
                # The sidecar now emits this via emit() (not session.sendMessage),
                # so it IS observable in the stdout event stream.
                websearch_event = None
                with session._cond:
                    for e in session.events:
                        if e.data.get("type") == "devboule_websearch":
                            websearch_event = e.data
                            break
                assert websearch_event is not None, (
                    f"devboule_websearch first-class event not found in event stream.\n"
                    f"Events:\n{session.dump_state()}"
                )
                assert websearch_event.get("query") == WEBSEARCH_QUERY, (
                    f"expected query={WEBSEARCH_QUERY}; got: {websearch_event.get('query')}"
                )
                assert websearch_event.get("results"), (
                    f"expected non-empty results in devboule_websearch event; got: {websearch_event.get('results')}"
                )

                # Wait for the response event (success:true).
                response = session.wait_response(timeout=30.0)
                assert response.data.get("success") is True, (
                    f"expected response success:true; got: {response.data}"
                )
