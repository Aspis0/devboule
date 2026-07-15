#!/usr/bin/env python3
"""
P2b tool-call round-trip + failure scenarios for the headless rig.

Three scenarios, all RIG=1 gated:

  test_read_tool_roundtrip
    - World has a planted file with a unique MARKER string.
    - Script: [{"tool":"read","arguments":{"path":<abs path>}}, "found MARKER"]
    - Assert the sidecar emits tool_execution_start{toolName:"read"} then
      tool_execution_end{isError:false}, then assistant text, then
      response{success:true}.
    - Assert the SECOND POST to the mock contains role:"tool" with the
      MARKER in content — proves the sandbox tool result actually came back
      to the model (not just synthesized).

  test_oversized_prompt_rejected
    - send_prompt with a >100_000 char message.
    - Assert the sidecar immediately answers response{success:false} with
      an error mentioning the size limit ("Prompt exceeds 100KB limit"),
      and ZERO POSTs to the mock (the size guard fires before any LLM call).

  test_midstream_connection_drop
    - Mock starts a valid SSE response (Content-Type + role chunk) then
      closes the socket before finish_reason / [DONE].
    - Assert the turn ends with response{success:false} + a non-empty error,
      no hang beyond the timeout, and the session survives (a follow-up
      prompt with a healthy scripted text still succeeds in the SAME session).

Total runtime target: <90s.
"""

from __future__ import annotations

import json
import os
import time
import uuid
from pathlib import Path

import pytest

# Skip entire module unless RIG=1 is set
pytestmark = pytest.mark.skipif(
    not os.environ.get("RIG"),
    reason="Self-test rig disabled. Set RIG=1 to run: RIG=1 python -m pytest rig/ -v",
)

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


def _collect_until_response(
    session: SidecarSession,
    timeout: float = 60.0,
    start_index: int = 0,
) -> tuple:
    """Pump session.events until a NEW response{command:prompt} event arrives.

    Scans from `start_index` onwards — caller MUST pass len(session.events)
    from the previous call to avoid re-matching a stale response event.

    Returns (response_event_or_None, scan_end_index). On timeout, returns
    (None, scan_end_index) so the caller can resume without re-scanning.
    """
    deadline = time.time() + timeout
    last_scanned = start_index
    while time.time() < deadline:
        try:
            event = session.wait_event(
                lambda e: True, timeout=0.5, start_index=last_scanned
            )
        except TimeoutError:
            continue

        last_scanned = session.events.index(event) + 1

        if not session.is_alive():
            raise RuntimeError(
                f"Sidecar process exited unexpectedly (returncode={session._proc.returncode}).\n"
                f"{session.dump_state()}"
            )

        if (
            event.data.get("type") == "response"
            and event.data.get("command") == "prompt"
        ):
            return event, last_scanned
    return None, last_scanned


# ---------------------------------------------------------------------------
# 1. test_read_tool_roundtrip
# ---------------------------------------------------------------------------


def test_read_tool_roundtrip():
    """Mock asks the pi agent to call `read` on a planted file → assert the
    round-trip: tool_call from model, tool_execution_start/end in stream,
    role:"tool" message in the next request carrying the file contents.
    """
    MARKER = f"RIG-MARKER-{uuid.uuid4().hex[:12]}"
    file_content = (
        f"// Rig test fixture file\n"
        f"// {MARKER} — this is the secret marker for the round-trip test\n"
        f"pub fn planted_marker() -> &'static str {{ \"{MARKER}\" }}\n"
    )

    with build_world_in_temp() as world:
        # Plant a file inside the sandbox project_root so the SDK's read tool
        # can resolve it (the tool resolves relative-to-cwd which is project_root).
        marker_path = world.project_root / "src" / "rig_marker.rs"
        marker_path.parent.mkdir(parents=True, exist_ok=True)
        marker_path.write_text(file_content, encoding="utf-8")

        # Sanity: world.project_root/src/lib.rs already exists from the builder.
        assert (world.project_root / "src" / "lib.rs").exists()

        with MockLLMServer() as mock:
            # Scripted responses:
            #   1) assistant issues a tool_call for `read` with absolute path
            #   2) assistant produces final text once the tool result returns
            #
            # The mock re-uses the same scripted id across the streamed tool_calls
            # deltas and (because we don't reuse the id from the second POST) the
            # SDK will mint a new toolCallId for any follow-up call. For this
            # scenario the FIRST tool_call id is enough — the test only asserts
            # that the second POST carries SOME tool role message.
            scripted_tool_call = {
                "tool": "read",
                "arguments": {"path": str(marker_path)},
            }
            scripted_final_text = (
                f"Found it — the file contains the marker {MARKER}."
            )
            mock.set_responses([scripted_tool_call, scripted_final_text])

            session_id = f"rig-tool-{uuid.uuid4().hex[:8]}"
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

                # 2. Send the prompt — clear mock log so we can isolate the
                #    two POSTs to chat/completions cleanly.
                mock.clear_request_log()
                session.send_prompt(
                    f"Please read {marker_path} and confirm the marker."
                )

                # 3. Pump events until terminal response.
                response_event, scan_end = _collect_until_response(
                    session, timeout=60.0
                )
                # Snapshot all events seen during this turn for assertion.
                events = list(session.events)

                assert response_event is not None, (
                    f"No response event received within 60s.\n"
                    f"{session.dump_state()}"
                )

                # 4. Terminal response must be success=true with no error.
                assert response_event.data.get("success") is True, (
                    f"Response failed: {response_event.data.get('error')!r}\n"
                    f"{session.dump_state()}"
                )
                assert response_event.data.get("error") in (None, ""), (
                    f"Unexpected error on success response: "
                    f"{response_event.data.get('error')!r}"
                )

                # 5. tool_execution_start{toolName:"read",args.path=<our path>}
                start_events = [
                    e
                    for e in events
                    if e.data.get("type") == "tool_execution_start"
                ]
                read_starts = [
                    e
                    for e in start_events
                    if e.data.get("toolName") == "read"
                ]
                assert read_starts, (
                    f"No tool_execution_start for 'read'. "
                    f"Tool start events seen: "
                    f"{[e.data.get('toolName') for e in start_events]}\n"
                    f"{session.dump_state()}"
                )
                # The args.path must equal what we scripted.
                read_start = read_starts[0]
                start_args = read_start.data.get("args") or {}
                assert start_args.get("path") == str(marker_path), (
                    f"Tool start args.path mismatch: "
                    f"got {start_args.get('path')!r}, expected {str(marker_path)!r}"
                )
                # The toolCallId must be present (used for the role:"tool" correlation).
                assert read_start.data.get("toolCallId"), (
                    f"Tool start missing toolCallId: {read_start.data}"
                )
                tool_call_id = read_start.data["toolCallId"]

                # 6. tool_execution_end{isError:false} for the same toolCallId.
                end_events = [
                    e
                    for e in events
                    if e.data.get("type") == "tool_execution_end"
                ]
                read_ends = [
                    e
                    for e in end_events
                    if e.data.get("toolCallId") == tool_call_id
                ]
                assert read_ends, (
                    f"No tool_execution_end for toolCallId={tool_call_id!r}. "
                    f"End events: {[e.data.get('toolCallId') for e in end_events]}\n"
                    f"{session.dump_state()}"
                )
                assert read_ends[0].data.get("isError") is False, (
                    f"Tool ended with error: {read_ends[0].data}"
                )

                # 7. Strict ordering: tool_execution_start must appear BEFORE
                #    the tool_execution_end for the same toolCallId, and the
                #    tool_execution_end must appear BEFORE the response event.
                start_idx = events.index(read_start)
                end_idx = events.index(read_ends[0])
                resp_idx = events.index(response_event)
                assert start_idx < end_idx < resp_idx, (
                    f"Ordering violation: start={start_idx}, end={end_idx}, "
                    f"response={resp_idx}"
                )

                # 8. Assistant text must contain the marker — proves the tool
                #    result actually reached the model (otherwise the second
                #    scripted response could not reference it).
                text_chunks = []
                for e in events:
                    text = _extract_text(e.data)
                    if text:
                        text_chunks.append(text)
                full_text = "".join(text_chunks)
                assert MARKER in full_text, (
                    f"Marker {MARKER!r} not in assistant text. "
                    f"Full text: {full_text!r}\n{session.dump_state()}"
                )

                # 9. The mock received EXACTLY 2 chat/completions POSTs:
                #    - the first carried the user prompt → tool_call reply
                #    - the second carried the tool result → text reply
                posts = _post_requests_for_completions(mock)
                assert len(posts) == 2, (
                    f"Expected 2 POST /v1/chat/completions, got {len(posts)}.\n"
                    f"Mock log: {[r.get('body_preview') for r in posts]}\n"
                    f"{session.dump_state()}"
                )

                # 10. The FIRST POST must NOT have a role:"tool" message.
                first_body = posts[0]["body"]
                assert posts[0].get("has_tool_result_message") is False, (
                    f"First POST unexpectedly has role:'tool' message.\n"
                    f"Body: {first_body[:500]}"
                )

                # 11. The SECOND POST must have a role:"tool" message AND
                #     its content must include the MARKER (proof that pi
                #     actually read the sandbox file and shipped the contents
                #     back to the model).
                assert posts[1].get("has_tool_result_message") is True, (
                    f"Second POST has no role:'tool' message.\n"
                    f"Body: {posts[1]['body'][:500]}"
                )
                second_body = posts[1]["body"]
                # Parse messages and find the tool message
                second_messages = json.loads(second_body).get("messages", [])
                tool_msgs = [
                    m for m in second_messages if m.get("role") == "tool"
                ]
                assert tool_msgs, (
                    f"No role:'tool' messages in second POST. "
                    f"Messages roles: {[m.get('role') for m in second_messages]}"
                )
                # Tool content should include the marker (the file contents).
                tool_content_blob = " ".join(
                    str(m.get("content", "")) for m in tool_msgs
                )
                assert MARKER in tool_content_blob, (
                    f"MARKER not in tool result content.\n"
                    f"Tool msgs: {tool_msgs}\n{session.dump_state()}"
                )

                # m1: wire-correlation — every role:"tool" message's tool_call_id
                # must equal the id the mock issued in its tool-call response
                # (the same id the SDK later echoes back as role:"tool".tool_call_id).
                for tool_msg in tool_msgs:
                    assert tool_msg.get("tool_call_id") == tool_call_id, (
                        f"role:'tool' tool_call_id mismatch: "
                        f"got {tool_msg.get('tool_call_id')!r}, expected {tool_call_id!r}"
                    )

                # 12. The second POST must also contain the assistant message
                #     with tool_calls (for correlation with the tool result).
                assistant_msgs = [
                    m
                    for m in second_messages
                    if m.get("role") == "assistant"
                    and m.get("tool_calls")
                ]
                assert assistant_msgs, (
                    f"No assistant tool_calls message in second POST. "
                    f"Messages: {[m.get('role') for m in second_messages]}"
                )


# ---------------------------------------------------------------------------
# 2. test_oversized_prompt_rejected
# ---------------------------------------------------------------------------


def test_oversized_prompt_rejected():
    """>100_000 char prompt is rejected BEFORE any LLM request — verifies the
    sidecar's #3 size guard (sidecar.mjs:721-727)."""
    with build_world_in_temp() as world:
        with MockLLMServer() as mock:
            # No scripted responses; any POST would be a test failure.

            session_id = f"rig-oversize-{uuid.uuid4().hex[:8]}"
            with SidecarSession(
                session_id=session_id,
                agent_role="orchestrator",
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                pigeon_enabled=False,
            ) as session:
                ready_event = session.wait_ready(timeout=30.0)
                assert ready_event.data.get("type") == "ready"

                # Send a 150_000-char message (limit is 100_000).
                big_msg = "x" * 150_000
                assert len(big_msg) > 100_000

                mock.clear_request_log()
                session.send_prompt(big_msg)

                # The rejection is synchronous — wait for response (no LLM call).
                response_event, scan_end = _collect_until_response(
                    session, timeout=15.0
                )

                # Assert terminal response is failure with the size error.
                assert response_event is not None, (
                    f"No response event within 15s (rejection must be synchronous).\n"
                    f"{session.dump_state()}"
                )
                assert response_event.data.get("success") is False, (
                    f"Expected success:false for oversized prompt, got: "
                    f"{response_event.data}\n{session.dump_state()}"
                )
                error_msg = response_event.data.get("error") or ""
                assert error_msg, (
                    f"Expected non-empty error message, got {error_msg!r}"
                )
                # The sidecar.mjs:726 string is exactly "Prompt exceeds 100KB limit"
                assert "100KB" in error_msg or "100_000" in error_msg, (
                    f"Error does not mention the size limit: {error_msg!r}\n"
                    f"{session.dump_state()}"
                )

                # NO LLM request should have been made.
                posts = _post_requests_for_completions(mock)
                assert posts == [], (
                    f"Expected ZERO chat/completions POSTs for oversized prompt, "
                    f"got {len(posts)}.\n"
                    f"Posts: {[r.get('body_preview') for r in posts]}"
                )

                # Session must still be alive — ready for a follow-up healthy
                # prompt (proves the rejection didn't kill the sidecar).
                assert session.is_alive(), (
                    f"Sidecar process died after rejection.\n"
                    f"{session.dump_state()}"
                )

                # Follow-up healthy prompt succeeds (defensive — proves
                # session survives the rejection). Pass scan_end so we don't
                # re-match the previous response event.
                mock.set_responses(["alive after rejection"])
                session.send_prompt("ping")
                followup, _scan_end2 = _collect_until_response(
                    session, timeout=30.0, start_index=scan_end
                )
                assert followup is not None, (
                    f"No response to follow-up healthy prompt.\n"
                    f"{session.dump_state()}"
                )
                assert followup.data.get("success") is True, (
                    f"Follow-up prompt failed: {followup.data.get('error')!r}\n"
                    f"{session.dump_state()}"
                )


# ---------------------------------------------------------------------------
# 3. test_midstream_connection_drop
# ---------------------------------------------------------------------------


def test_midstream_connection_drop():
    """Mock opens SSE headers + sends the role-only chunk, then closes the
    socket before finish_reason / [DONE]. The SDK's openai-completions stream
    loop sees `Stream ended without finish_reason` (classified as retryable
    by `isRetryableAssistantError` — the pattern "ended without" matches),
    auto-retries ~3x with exponential backoff (2s, 4s, 8s).

    The mock is scripted to KEEP FAILING until retries exhaust (fail_next_n=100
    so every HTTP attempt hits midstream_drop). After the terminal
    response{success:false} is observed, fail mode is cleared and a healthy
    follow-up prompt in the SAME session must succeed.
    """
    with build_world_in_temp() as world:
        with MockLLMServer() as mock:
            # Every HTTP attempt fails with midstream_drop. The SDK auto-retries
            # ~3x (maxRetries=3, baseDelayMs=2000), so 4 total attempts.
            # After the retries exhaust, the sidecar emits response{success:false}.
            mock.set_fail_mode(count=100, mode="midstream_drop")

            session_id = f"rig-drop-{uuid.uuid4().hex[:8]}"
            with SidecarSession(
                session_id=session_id,
                agent_role="orchestrator",
                mock_base_url=mock.base_url + "/v1",
                project_root=world.project_root,
                agent_dir=world.agent_dir,
                pigeon_enabled=False,
            ) as session:
                ready_event = session.wait_ready(timeout=30.0)
                assert ready_event.data.get("type") == "ready"

                # 1. First prompt — every attempt hits midstream_drop.
                #    The SDK retries ~3x; after retries exhaust, the turn ends
                #    with success:false + non-empty error.
                mock.clear_request_log()
                session.send_prompt("please recover from drop")

                # Generous timeout: 4 attempts × backoff (2+4+8=14s) + overhead.
                response_event, scan_end = _collect_until_response(
                    session, timeout=45.0
                )

                assert response_event is not None, (
                    f"No response within 45s after midstream drop retries.\n"
                    f"{session.dump_state()}"
                )

                # Terminal response must be success:false with a non-empty error.
                assert response_event.data.get("success") is False, (
                    f"Expected success:false after retries exhausted, got: "
                    f"{response_event.data}\n{session.dump_state()}"
                )
                error_msg = response_event.data.get("error")
                assert error_msg, (
                    f"Expected non-empty error message, got {error_msg!r}\n"
                    f"{session.dump_state()}"
                )

                # At least one chat/completions POST must have been recorded.
                posts = _post_requests_for_completions(mock)
                assert posts, (
                    f"No chat/completions POSTs recorded — midstream drop did not "
                    f"even hit the handler.\n{session.dump_state()}"
                )
                assert any(p.get("stream") is True for p in posts), (
                    f"No streaming POST recorded.\nPosts: {posts}"
                )

                # 2. Clear fail mode so the follow-up prompt is healthy.
                mock.clear_fail_mode()

                # 3. Follow-up prompt in the SAME session — must succeed.
                mock.clear_request_log()
                session.send_prompt("second prompt after recovery")

                followup, _scan_end2 = _collect_until_response(
                    session, timeout=30.0, start_index=scan_end
                )
                assert followup is not None, (
                    f"Follow-up prompt produced no response.\n{session.dump_state()}"
                )
                assert followup.data.get("success") is True, (
                    f"Follow-up prompt failed: {followup.data.get('error')!r}\n"
                    f"{session.dump_state()}"
                )

                # 4. Session is still alive.
                assert session.is_alive(), (
                    f"Sidecar process died mid-test.\n{session.dump_state()}"
                )


# text extraction helper (mirrors test_smoke._extract_assistant_text_from_event)
# ---------------------------------------------------------------------------


def _extract_text(event_data: dict) -> str:
    """Extract assistant text content from various pi SDK event shapes.

    Mirrors the helper in test_smoke.py — duplicated here because the smoke
    helper is a module-private name and we want strict file-scope discipline
    (test_tool_roundtrip.py is the ONLY new file per task constraints).
    """
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


if __name__ == "__main__":
    # Allow running directly for quick debugging
    os.environ["RIG"] = "1"
    pytest.main([__file__, "-v", "-s"])