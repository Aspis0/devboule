#!/usr/bin/env python3
"""
P5c: pi coder lane parity + gap pins.

Three cells, all RIG=1 gated:

  test_mini_coder_role_turn_roundtrip
    - Spawn sidecar with agent_role="mini-coder" against a plain-text mock.
    - Assert user echo before assistant, response{success:true}, non-empty text,
      and _devboule.agentRole enrichment == "mini-coder".

  test_censor_review_trigger_on_rs_write
    - Sidecar has a censor hook: on tool_execution_start of write/edit targeting
      a .rs file, it queues the file; at agent_end it emits a first-class
      devboule_censor_review event (env-gated by DEVBOULE_CENSOR_REVIEW_ENABLED,
      NOT role-gated).
    - Script the mock: tool_call to `write` targeting a planted .rs, then a
      final plain-text turn. Assert the tool executed (isError falsy) and the
      devboule_censor_review event includes the .rs path.

  test_steer_pi_coder_id_pins_not_found
    - GAP PIN (product gap, intentionally NOT fixed yet): steering only targets
      directive rows in .aspis-agents.json (dispatch_steer_mini_coder returns
      {directiveId, status:"not_found"} when the id matches no directive). A
      pi-sidecar coder session id is not a directive, so steering it goes nowhere.
    - Pure MCP (no sidecar): seed a world, register a forged ORCHESTRATOR session
      (steer_mini_coder is role-gated), call steer_mini_coder with directive_id
      and a message. Assert status == "not_found".
"""

from __future__ import annotations

import os
import tempfile
import uuid
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Gate: only run when RIG=1
# ---------------------------------------------------------------------------
if os.environ.get("RIG") != "1":
    pytest.skip(
        "RIG=1 required; skipping pi coder lane parity tests",
        allow_module_level=True,
    )

# ---------------------------------------------------------------------------
# Imports from our own rig modules
# ---------------------------------------------------------------------------
from rig.mock_llm import MockLLMServer  # noqa: E402
from rig.world import build_world_in_temp  # noqa: E402
from rig.sidecar_driver import SidecarSession  # noqa: E402
from rig.mcp_client import McpStdioClient  # noqa: E402
from rig.world import make_projects_dir, forge_agent_launch  # noqa: E402

# Repo root (two levels up from rig/)
REPO_ROOT = Path(__file__).resolve().parent.parent


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


def _extract_text(event_data: dict) -> str:
    """Extract assistant text content from various pi SDK event shapes."""
    etype = event_data.get("type")

    if etype == "message_update":
        ame = event_data.get("assistantMessageEvent", {})
        if ame:
            return _extract_text(ame)

    if "message" in event_data:
        msg = event_data["message"]
        if (
            isinstance(msg, dict)
            and msg.get("role") == "assistant"
            and "content" in msg
        ):
            content = msg["content"]
            if isinstance(content, list):
                parts = []
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "text":
                        parts.append(block.get("text", ""))
                return "".join(parts)
            elif isinstance(content, str):
                return content

    if etype == "text_delta":
        return event_data.get("delta", "")

    if etype == "assistant":
        content = event_data.get("content")
        if isinstance(content, list):
            parts = []
            for block in content:
                if isinstance(block, dict) and block.get("type") == "text":
                    parts.append(block.get("text", ""))
            return "".join(parts)

    return ""


# ---------------------------------------------------------------------------
# 1. test_mini_coder_role_turn_roundtrip
# ---------------------------------------------------------------------------


def test_mini_coder_role_turn_roundtrip():
    """Parity twin of the orchestrator ciao-smoke: mini-coder role round-trip.

    Assert:
      - user echo before assistant text
      - response event with success:true
      - non-empty assistant text
      - _devboule.agentRole enrichment == "mini-coder"
    """
    EXPECTED_ROLE = "mini-coder"
    ASSISTANT_TEXT = "I am a mini-coder, ready to help."

    with build_world_in_temp() as world:
        with MockLLMServer() as mock:
            # Scripted response: plain text (no tool calls).
            mock.set_responses([ASSISTANT_TEXT])

            session_id = f"rig-mini-{uuid.uuid4().hex[:8]}"
            with SidecarSession(
                session_id=session_id,
                agent_role=EXPECTED_ROLE,
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                pigeon_enabled=False,
            ) as session:
                # 1. Wait for ready.
                ready_event = session.wait_ready(timeout=30.0)
                assert ready_event.data.get("type") == "ready"

                # 2. Clear mock log, send a prompt.
                mock.clear_request_log()
                session.send_prompt("Introduce yourself.")

                # 3. Wait for the terminal response.
                response_event = session.wait_response(timeout=60.0)
                events = list(session.events)

                # 4. Assert terminal response success:true.
                assert response_event.data.get("success") is True, (
                    f"Expected success:true; got: {response_event.data}\n"
                    f"{session.dump_state()}"
                )

                # 5. Assert user echo appears BEFORE assistant text.
                # The user echo arrives as message_start/message_end events whose
                # message.role is "user" (there is no bare type=="message" event).
                user_echo_indices = [
                    i
                    for i, e in enumerate(events)
                    if e.data.get("type") in ("message_start", "message_end")
                    and (e.data.get("message") or {}).get("role") == "user"
                ]
                assistant_text_indices = [
                    i
                    for i, e in enumerate(events)
                    if _extract_text(e.data)
                ]
                assert user_echo_indices, (
                    f"No user echo event found.\n{session.dump_state()}"
                )
                assert assistant_text_indices, (
                    f"No assistant text found.\n{session.dump_state()}"
                )
                assert user_echo_indices[0] < assistant_text_indices[0], (
                    f"User echo must precede assistant text.\n{session.dump_state()}"
                )

                # 6. Assert non-empty assistant text.
                full_text = "".join(_extract_text(e.data) for e in events)
                assert full_text, (
                    f"Assistant text is empty.\n{session.dump_state()}"
                )
                # The scripted text must be present (proves the mock was hit).
                assert ASSISTANT_TEXT in full_text, (
                    f"Expected assistant text {ASSISTANT_TEXT!r}; got: {full_text!r}\n"
                    f"{session.dump_state()}"
                )

                # 7. Assert _devboule.agentRole enrichment == "mini-coder" on events.
                devboule_events = [
                    e for e in events if e.data.get("_devboule")
                ]
                assert devboule_events, (
                    f"No events with _devboule enrichment.\n{session.dump_state()}"
                )
                for dev_event in devboule_events:
                    agent_role = dev_event.data["_devboule"].get("agentRole")
                    assert agent_role == EXPECTED_ROLE, (
                        f"_devboule.agentRole mismatch: got {agent_role!r}, "
                        f"expected {EXPECTED_ROLE!r}\n{session.dump_state()}"
                    )

                # 8. Exactly 1 POST to the mock (plain text, no tool calls).
                posts = _post_requests_for_completions(mock)
                assert len(posts) == 1, (
                    f"Expected 1 POST /v1/chat/completions, got {len(posts)}.\n"
                    f"{session.dump_state()}"
                )


# ---------------------------------------------------------------------------
# 2. test_censor_review_trigger_on_rs_write
# ---------------------------------------------------------------------------


def test_censor_review_trigger_on_rs_write():
    """Censor hook: write/edit targeting a .rs file -> devboule_censor_review event.

    The sidecar censor hook (sidecar.mjs ~line 471-595 + triggerCensorReview
    ~line 810-895) queues .rs files on tool_execution_start of write/edit, then
    at agent_end emits a first-class devboule_censor_review event. It is
    env-gated by DEVBOULE_CENSOR_REVIEW_ENABLED (default enabled), NOT role-gated.

    triggerCensorReview does NOT start another prompt turn -- it only emits the
    event (actual review execution deferred to Phase 5). So the mock needs only
    the tool_call response + a final text response.
    """
    MARKER = f"RIG-CENSOR-{uuid.uuid4().hex[:12]}"
    RS_CONTENT = (
        f"// Rig test fixture -- {MARKER}\n"
        f"pub fn hello() -> &'static str {{ \"hello\" }}\n"
    )

    with build_world_in_temp() as world:
        # Plant a .rs file inside the sandbox project_root.
        rs_path = world.project_root / "src" / "rig_censor.rs"
        rs_path.parent.mkdir(parents=True, exist_ok=True)
        rs_path.write_text(RS_CONTENT, encoding="utf-8")

        with MockLLMServer() as mock:
            # Scripted responses:
            #   1) assistant issues a tool_call for `write` targeting the .rs file
            #   2) assistant produces final text once the tool result returns
            #
            # Shape copied exactly from test_tool_roundtrip.py.
            scripted_tool_call = {
                "tool": "write",
                "arguments": {"path": str(rs_path), "content": RS_CONTENT},
            }
            scripted_final_text = (
                f"Wrote the file. Marker: {MARKER}."
            )
            mock.set_responses([scripted_tool_call, scripted_final_text])

            session_id = f"rig-censor-{uuid.uuid4().hex[:8]}"
            with SidecarSession(
                session_id=session_id,
                agent_role="mini-coder",
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                pigeon_enabled=False,
                env_overrides={
                    # Explicitly enable the censor hook (default is on, but be explicit).
                    "DEVBOULE_CENSOR_REVIEW_ENABLED": "true",
                },
            ) as session:
                # 1. Wait for ready.
                ready_event = session.wait_ready(timeout=30.0)
                assert ready_event.data.get("type") == "ready"

                # 2. Clear mock log, send a prompt.
                mock.clear_request_log()
                session.send_prompt(
                    f"Write a hello function to {rs_path} and confirm."
                )

                # 3. Wait for the terminal response.
                response_event = session.wait_response(timeout=60.0)

                # 3b. The censor hook defers its trigger past the prompt turn
                # (handleCensorAgentEnd uses setTimeout(0) at agent_end and
                # triggerCensorReview sleeps CENSOR_REVIEW_DELAY_MS=500ms), so
                # devboule_censor_review lands AFTER the response event. Wait
                # for it with a bounded timeout instead of snapshotting now.
                session.wait_event(
                    lambda e: e.data.get("type") == "devboule_censor_review",
                    timeout=10.0,
                )
                events = list(session.events)

                # 4. Assert tool_execution_start for write.
                write_starts = [
                    e
                    for e in events
                    if e.data.get("type") == "tool_execution_start"
                    and e.data.get("toolName") == "write"
                ]
                assert write_starts, (
                    f"No tool_execution_start for 'write'.\n{session.dump_state()}"
                )
                write_start = write_starts[0]
                start_args = write_start.data.get("args") or {}
                assert start_args.get("path") == str(rs_path), (
                    f"Tool start args.path mismatch: "
                    f"got {start_args.get('path')!r}, expected {str(rs_path)!r}"
                )
                tool_call_id = write_start.data.get("toolCallId")
                assert tool_call_id, (
                    f"Tool start missing toolCallId: {write_start.data}"
                )

                # 5. Assert tool_execution_end for write with isError:false.
                write_ends = [
                    e
                    for e in events
                    if e.data.get("type") == "tool_execution_end"
                    and e.data.get("toolCallId") == tool_call_id
                ]
                assert write_ends, (
                    f"No tool_execution_end for toolCallId={tool_call_id!r}.\n"
                    f"{session.dump_state()}"
                )
                assert write_ends[0].data.get("isError") is False, (
                    f"Tool ended with error: {write_ends[0].data}"
                )

                # 6. Assert devboule_censor_review event with the .rs path.
                censor_events = [
                    e for e in events if e.data.get("type") == "devboule_censor_review"
                ]
                assert censor_events, (
                    f"No devboule_censor_review event found.\n{session.dump_state()}"
                )
                censor_event = censor_events[0]
                files = censor_event.data.get("files") or []
                assert str(rs_path) in files, (
                    f"Censor review files missing {rs_path!r}; got {files}\n"
                    f"{session.dump_state()}"
                )

                # 7. Assert terminal response success:true.
                assert response_event.data.get("success") is True, (
                    f"Expected success:true; got: {response_event.data}\n"
                    f"{session.dump_state()}"
                )

                # 8. Exactly 2 POSTs to the mock (tool_call + text).
                posts = _post_requests_for_completions(mock)
                assert len(posts) == 2, (
                    f"Expected 2 POST /v1/chat/completions, got {len(posts)}.\n"
                    f"{session.dump_state()}"
                )


# ---------------------------------------------------------------------------
# 3. test_steer_pi_coder_id_pins_not_found
# ---------------------------------------------------------------------------


def test_steer_pi_coder_id_pins_not_found():
    """GAP PIN (product gap, intentionally NOT fixed yet): steering a
    pi-sidecar coder session id goes nowhere -- dispatch_steer_mini_coder
    returns {directiveId, status:"not_found"} when the id matches no
    directive.

    steering only targets directive rows in .aspis-agents.json
    (dispatch_steer_mini_coder, oracle/server/aspis_mcp.py:6726). A
    pi-sidecar session id is NOT a directive, so steering it is explicit
    not_found, not silent.

    This test registers a forged ORCHESTRATOR session (steer_mini_coder is
    role-gated -- see ROLE_ALLOWED_TOOLS in role_rules.json) and calls
    steer_mini_coder with a bogus directive_id. Assert status == "not_found".

    P5c gap #2 -- routing fix pending owner design decision.
    """
    BOGUS_DIRECTIVE_ID = "pi-mini-coder-rigpin"
    STEER_MESSAGE = "fix the off-by-one bug"

    with tempfile.TemporaryDirectory(prefix="rig-steer-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        # Forge a launch token for the orchestrator (steer_mini_coder is
        # role-gated to coder/orchestrator per role_rules.json).
        orch_token = forge_agent_launch(projects_dir, "rig-orchestrator", "orchestrator")

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # ---- Step 1: register the orchestrator ----
            reg, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": "rig-orchestrator",
                    "role": "orchestrator",
                    "model": "rig-model",
                    "launch_token": orch_token,
                },
                timeout=15,
            )
            session_token = reg["sessionToken"]

            # ---- Step 2: steer a bogus directive_id -> not_found ----
            steer_result, _ = client.call_tool(
                "steer_mini_coder",
                {
                    "agent_id": "rig-orchestrator",
                    "role": "orchestrator",
                    "directive_id": BOGUS_DIRECTIVE_ID,
                    "message": STEER_MESSAGE,
                    "session_token": session_token,
                },
                timeout=10,
            )

            # Assert the explicit not_found response.
            assert steer_result.get("directiveId") == BOGUS_DIRECTIVE_ID, (
                f"Unexpected directiveId: {steer_result}"
            )
            assert steer_result.get("status") == "not_found", (
                f"Expected status='not_found'; got: {steer_result}\n"
                f"This is P5c gap #2: steering a pi-sidecar coder session id "
                f"goes nowhere because dispatch_steer_mini_coder only targets "
                f"directive rows in .aspis-agents.json, not session ids. "
                f"Routing fix pending owner design decision."
            )


if __name__ == "__main__":
    # Allow running directly for quick debugging
    os.environ["RIG"] = "1"
    pytest.main([__file__, "-v", "-s"])
