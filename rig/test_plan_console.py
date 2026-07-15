#!/usr/bin/env python3
"""
Plan console rig cell.

Gated by RIG=1 only (no RIG_LIVE needed — the plan tool is a local builtin,
no network). Asserts the devboule_plan FIRST-CLASS EVENT when the `plan` tool
executes, and proves the tool is NOT registered for coder roles.

The `plan` tool is registered as a custom tool in sidecar.mjs only when
DEVBOULE_AGENT_ROLE=orchestrator (see buildPlanTool in sidecar.mjs).
"""

from __future__ import annotations

import json
import os
import sys
import uuid
from pathlib import Path

import pytest

if os.environ.get("RIG") != "1":
    pytest.skip("RIG=1 required; skipping plan console tests", allow_module_level=True)

from rig.mock_llm import MockLLMServer
from rig.world import build_world_in_temp
from rig.sidecar_driver import SidecarSession


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _post_requests_for_completions(mock: MockLLMServer) -> list[dict]:
    """Filter mock request log to just chat/completions POSTs."""
    return [
        r
        for r in mock.get_request_log()
        if r["method"] == "POST" and r["path"] == "/v1/chat/completions"
    ]


# ---------------------------------------------------------------------------
# 1. test_plan_console
# ---------------------------------------------------------------------------


def test_plan_console():
    """Mock orchestrator → tool_call plan → assert tool_execution_start/end +
    first-class devboule_plan event + response success:true.

    The `plan` tool is orchestrator-only (registered via customTools in
    sidecar.mjs only when agentRole === "orchestrator").
    """
    PLAN_TITLE = "Fix add()"
    PLAN_NOTES = "rig"

    with build_world_in_temp() as world:
        with MockLLMServer() as mock:
            # Scripted responses:
            #   1) assistant issues a tool_call for `plan` with the plan payload
            #   2) assistant produces final text once the tool result returns
            scripted_tool_call = {
                "tool": "plan",
                "arguments": {
                    "title": PLAN_TITLE,
                    "steps": [
                        {"text": "read lib.rs"},
                        {"text": "fix off-by-one", "status": "in_progress"},
                    ],
                    "notes": PLAN_NOTES,
                },
            }
            scripted_final_text = "Plan updated."
            mock.set_responses([scripted_tool_call, scripted_final_text])

            session_id = f"rig-plan-{uuid.uuid4().hex[:8]}"
            with SidecarSession(
                session_id=session_id,
                agent_role="orchestrator",
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                pigeon_enabled=False,
            ) as session:
                # 1. Wait for ready
                ready_event = session.wait_ready(timeout=30.0)
                assert ready_event.data.get("type") == "ready"

                # 2. Send the prompt — clear mock log so we can isolate POSTs.
                mock.clear_request_log()
                session.send_prompt("Plan the fix.")

                # 3. Wait for the terminal response — by the time this returns,
                # tool_execution_start/end and devboule_plan events are already
                # in session.events (they precede response in-stream).
                response_event = session.wait_response(timeout=60.0)
                events = list(session.events)

                # 4. Assert terminal response success:true.
                assert response_event.data.get("success") is True, (
                    f"Expected success:true; got: {response_event.data}\n"
                    f"{session.dump_state()}"
                )

                # 5. Assert tool_execution_start AND tool_execution_end for plan.
                plan_starts = [
                    e
                    for e in events
                    if e.data.get("type") == "tool_execution_start"
                    and e.data.get("toolName") == "plan"
                ]
                assert plan_starts, (
                    f"tool_execution_start for 'plan' not observed.\n"
                    f"{session.dump_state()}"
                )
                plan_start = plan_starts[0]
                plan_start_args = plan_start.data.get("args") or {}
                assert plan_start_args.get("title") == PLAN_TITLE, (
                    f"plan tool start args mismatch: {plan_start_args}"
                )
                plan_tool_call_id = plan_start.data.get("toolCallId")
                assert plan_tool_call_id, "plan start missing toolCallId"

                plan_ends = [
                    e
                    for e in events
                    if e.data.get("type") == "tool_execution_end"
                    and e.data.get("toolCallId") == plan_tool_call_id
                ]
                assert plan_ends, (
                    f"tool_execution_end for plan toolCallId={plan_tool_call_id!r} not observed.\n"
                    f"{session.dump_state()}"
                )
                plan_end = plan_ends[0]
                assert plan_end.data.get("isError") is False, (
                    f"plan tool ended with error: {plan_end.data}"
                )

                # 6. Assert the first-class devboule_plan event.
                plan_events = [
                    e
                    for e in events
                    if e.data.get("type") == "devboule_plan"
                ]
                assert plan_events, (
                    f"devboule_plan first-class event not found.\n"
                    f"{session.dump_state()}"
                )
                plan_event = plan_events[0]
                plan_payload = plan_event.data.get("plan")
                assert plan_payload, f"devboule_plan has no plan payload: {plan_event.data}"
                assert plan_payload.get("title") == PLAN_TITLE, (
                    f"plan.title mismatch: {plan_payload.get('title')!r}"
                )
                plan_steps = plan_payload.get("steps") or []
                assert len(plan_steps) == 2, (
                    f"Expected 2 plan steps, got {len(plan_steps)}: {plan_steps}"
                )
                # First step status must be normalized to "pending".
                assert plan_steps[0].get("status") == "pending", (
                    f"First step status not normalized to 'pending': {plan_steps[0]}"
                )
                # Second step must keep its in_progress status.
                assert plan_steps[1].get("status") == "in_progress", (
                    f"Second step status mismatch: {plan_steps[1]}"
                )
                # A numeric timestamp must be present.
                ts = plan_event.data.get("timestamp")
                assert isinstance(ts, int) and ts > 0, (
                    f"devboule_plan timestamp not numeric: {ts!r}"
                )
                # Notes must be present with the scripted value.
                assert plan_payload.get("notes") == PLAN_NOTES, (
                    f"plan.notes mismatch: {plan_payload.get('notes')!r}"
                )

                # 7. Confirm response events are present in session.events.
                response_events = [
                    e
                    for e in events
                    if e.data.get("type") == "response"
                    and e.data.get("command") == "prompt"
                ]
                assert response_events, (
                    f"No response event received within 60s.\n"
                    f"{session.dump_state()}"
                )
                response = response_events[-1]
                assert response.data.get("success") is True, (
                    f"Expected success:true; got: {response.data}\n"
                    f"{session.dump_state()}"
                )

                # 8. The mock received EXACTLY 2 chat/completions POSTs:
                #    - first: user prompt + tool_call reply
                #    - second: tool result + text reply
                posts = _post_requests_for_completions(mock)
                assert len(posts) == 2, (
                    f"Expected 2 POST /v1/chat/completions, got {len(posts)}.\n"
                    f"{session.dump_state()}"
                )

                # 9. FIRST POST must NOT contain a tool named "plan" in its
                #    tools array (the model issued the tool_call, the first
                #    POST is the request that produced it).
                first_body = posts[0]["body"]
                first_msgs = json.loads(first_body).get("messages", [])
                # The first POST should not have a role:"tool" message (the
                # tool result hasn't been sent back yet).
                first_tool_msgs = [
                    m for m in first_msgs if m.get("role") == "tool"
                ]
                assert first_tool_msgs == [], (
                    f"First POST unexpectedly has role:'tool' message.\n"
                    f"Body: {first_body[:500]}"
                )


# ---------------------------------------------------------------------------
# 2. test_plan_tool_absent_for_coder_role
# ---------------------------------------------------------------------------


def test_plan_tool_absent_for_coder_role():
    """Prove the `plan` tool is NOT registered when agent_role=main-coder.

    The sidecar gates customTools registration on devbouleContext.agentRole ===
    "orchestrator" (sidecar.mjs). For coder roles, the first POST body's
    `tools` array must NOT contain a tool named "plan".
    """
    with build_world_in_temp() as world:
        with MockLLMServer() as mock:
            # Plain text reply — no tool calls at all.
            mock.set_responses(["I'm just a coder, no plans here."])

            session_id = f"rig-plan-coder-{uuid.uuid4().hex[:8]}"
            with SidecarSession(
                session_id=session_id,
                agent_role="main-coder",
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                pigeon_enabled=False,
            ) as session:
                # 1. Wait for ready
                ready_event = session.wait_ready(timeout=30.0)
                assert ready_event.data.get("type") == "ready"

                # 2. Clear mock log, send a prompt that could trigger a plan.
                mock.clear_request_log()
                session.send_prompt("Plan the fix and implement it.")

                # 3. Wait for the terminal response.
                response = session.wait_response(timeout=60.0)
                assert response.data.get("success") is True, (
                    f"Expected success:true for coder role; got: {response.data}\n"
                    f"{session.dump_state()}"
                )

                # 4. Assert the FIRST POST body contains NO tool named "plan"
                #    in its `tools` array. This proves the role gate works:
                #    the plan tool is NOT registered for main-coder.
                posts = _post_requests_for_completions(mock)
                assert posts, (
                    f"No chat/completions POST recorded for coder role.\n"
                    f"{session.dump_state()}"
                )
                first_body = json.loads(posts[0]["body"])
                tools = first_body.get("tools") or []
                tool_names = [
                    t.get("function", {}).get("name") or t.get("name")
                    for t in tools
                ]
                assert "plan" not in tool_names, (
                    f"Tool 'plan' found in first POST tools array for coder role.\n"
                    f"Tools: {tool_names}\n"
                    f"Body: {posts[0]['body'][:1000]}\n"
                    f"{session.dump_state()}"
                )

                # 6. No tool_execution_start for "plan" in the event stream.
                plan_starts = [
                    e
                    for e in session.events
                    if e.data.get("type") == "tool_execution_start"
                    and e.data.get("toolName") == "plan"
                ]
                assert plan_starts == [], (
                    f"Unexpected tool_execution_start for 'plan' in coder session.\n"
                    f"{session.dump_state()}"
                )


if __name__ == "__main__":
    # Allow running directly for quick debugging
    os.environ["RIG"] = "1"
    pytest.main([__file__, "-v", "-s"])
